//! Fast Copy — Windows 11 modern ("top-level") File Explorer context menu.
//!
//! A COM in-proc server (cdylib) implementing `IExplorerCommand`. It is registered by the
//! sparse package (`packaging/windows/sparse/AppxManifest.xml`), which gives the DLL the
//! package identity Windows 11 requires for the new context menu. The classic menu
//! ("Show more options", Windows 10) is handled separately by registry verbs written by
//! `fc_core::shell::install` and does not need this DLL.
//!
//! Two COM classes are exposed, both served by this DLL:
//!
//! | CLSID                                    | Registered for              | Sub-menu                         |
//! |------------------------------------------|-----------------------------|----------------------------------|
//! | `{FD3E33D7-C9C3-48BA-BEB5-BBE21DCEDFC1}` | `*`, `Directory`            | Copy / Move (+ Paste on a folder) |
//! | `{2F9A8E37-3C38-4E7A-8B4C-61AE88A3E347}` | `Directory\Background`      | Paste here                       |
//!
//! Each class returns a root command "Fast Copy" (`ECF_HASSUBCOMMANDS`) whose
//! `IEnumExplorerCommand` yields the localized (FR/EN) sub-commands. Invoking one launches
//! `fast-copy.exe` (located next to this DLL) as
//! `fast-copy.exe --shell copy|move|paste <paths…>`; for large selections (> 20 items or a
//! command line > 8000 chars) the paths are written to a UTF-8 list file (one absolute path
//! per line, no BOM, `%TEMP%\fastcopy-shell-*.lst`) and `@<listfile>` is passed instead.
//! The GUI owns the list file once launched (it should delete it after reading).
//!
//! Robustness rules (this code runs inside explorer.exe):
//! * no `unwrap`/`expect`/indexing that can panic (release builds use `panic = "abort"`,
//!   so a panic would kill Explorer);
//! * every failure is turned into an `HRESULT` (`E_FAIL` for `Invoke`), states fall back to
//!   "hidden";
//! * no work in `DllMain` (there is none), the DLL path is resolved lazily and cached.

#![cfg(windows)]
#![allow(non_snake_case)]

use core::ffi::c_void;
use std::io::Write as _;
use std::sync::atomic::{AtomicIsize, AtomicUsize, Ordering};
use std::sync::OnceLock;

use windows::core::{implement, w, Interface, GUID, HRESULT, PCWSTR, PWSTR};
use windows::Win32::Foundation::{
    BOOL, CLASS_E_CLASSNOTAVAILABLE, CLASS_E_NOAGGREGATION, E_FAIL, E_INVALIDARG, E_NOTIMPL,
    E_POINTER, HMODULE, S_FALSE, S_OK,
};
use windows::Win32::Globalization::GetUserDefaultUILanguage;
use windows::Win32::System::Com::{CoTaskMemFree, IBindCtx, IClassFactory, IClassFactory_Impl};
use windows::Win32::System::DataExchange::IsClipboardFormatAvailable;
use windows::Win32::System::LibraryLoader::{
    GetModuleFileNameW, GetModuleHandleExW, GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS,
    GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
};
use windows::Win32::System::Ole::CF_HDROP;
use windows::Win32::System::SystemServices::{
    SFGAO_FILESYSTEM, SFGAO_FLAGS, SFGAO_FOLDER, SFGAO_STREAM,
};
use windows::Win32::System::Threading::{
    CreateProcessW, CREATE_DEFAULT_ERROR_MODE, PROCESS_INFORMATION, STARTUPINFOW,
};
use windows::Win32::UI::Shell::{
    IEnumExplorerCommand, IEnumExplorerCommand_Impl, IExplorerCommand, IExplorerCommand_Impl,
    IShellItemArray, SHStrDupW, ECF_DEFAULT, ECF_HASSUBCOMMANDS, ECS_ENABLED, ECS_HIDDEN,
    SIATTRIBFLAGS_AND, SIGDN_FILESYSPATH,
};
use windows::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow;

// ---------------------------------------------------------------------------------------
// Identifiers
// ---------------------------------------------------------------------------------------

/// CLSID of the command shown on files and folders (`*`, `Directory`).
pub const CLSID_FASTCOPY_ITEMS: GUID = GUID::from_u128(0xfd3e33d7_c9c3_48ba_beb5_bbe21dcedfc1);
/// CLSID of the command shown on a folder background (`Directory\Background`).
pub const CLSID_FASTCOPY_BACKGROUND: GUID = GUID::from_u128(0x2f9a8e37_3c38_4e7a_8b4c_61ae88a3e347);

// Flat, top-level commands (one class per entry — no "Fast Copy ▸" sub-menu). These are the
// classes registered by the sparse package; the two root classes above remain available.
/// "Copier avec Fast Copy" (`*`, `Directory`).
pub const CLSID_FASTCOPY_COPY: GUID = GUID::from_u128(0x5ac697db_2a91_47ae_9df4_83d916bf7025);
/// "Déplacer avec Fast Copy" (`*`, `Directory`).
pub const CLSID_FASTCOPY_MOVE: GUID = GUID::from_u128(0x11ffd5d3_8a78_4a65_a664_449cd515a7a0);
/// "Coller ici avec Fast Copy" on a selected folder (`Directory`).
pub const CLSID_FASTCOPY_PASTE_INTO: GUID = GUID::from_u128(0x0c57fa8b_4d91_4528_b75d_62e16cceb676);
/// "Coller ici avec Fast Copy" on a folder background (`Directory\Background`).
pub const CLSID_FASTCOPY_PASTE_HERE: GUID = GUID::from_u128(0x1cc48706_c26d_4b46_b786_28a0942ab45e);

/// Above this many items (or [`MAX_CMDLINE`] UTF-16 units), paths go through a list file.
const MAX_INLINE_ITEMS: u32 = 20;
const MAX_CMDLINE: usize = 8000;

// ---------------------------------------------------------------------------------------
// DLL lifetime bookkeeping
// ---------------------------------------------------------------------------------------

/// Live COM objects + `LockServer` locks. `DllCanUnloadNow` returns `S_OK` only at zero.
static LIVE: AtomicIsize = AtomicIsize::new(0);

/// Embedded in every COM object: counts it as alive for `DllCanUnloadNow`.
struct LiveGuard;
impl LiveGuard {
    fn new() -> Self {
        LIVE.fetch_add(1, Ordering::SeqCst);
        LiveGuard
    }
}
impl Drop for LiveGuard {
    fn drop(&mut self) {
        LIVE.fetch_sub(1, Ordering::SeqCst);
    }
}

// ---------------------------------------------------------------------------------------
// Localization
// ---------------------------------------------------------------------------------------

fn is_french() -> bool {
    // PRIMARYLANGID(langid) == LANG_FRENCH (0x0C)
    // SAFETY: no arguments, cannot fail.
    (unsafe { GetUserDefaultUILanguage() } & 0x3ff) == 0x0c
}

// ---------------------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    /// Root "Fast Copy" for selected files/folders.
    RootItems,
    /// Root "Fast Copy" for a folder background.
    RootBackground,
    Copy,
    Move,
    /// "Paste here" on a single selected folder.
    PasteInto,
    /// "Paste here" on a folder background.
    PasteHere,
}

impl Kind {
    fn title(self) -> PCWSTR {
        let fr = is_french();
        match self {
            Kind::RootItems | Kind::RootBackground => w!("Fast Copy"),
            Kind::Copy if fr => w!("Copier avec Fast Copy"),
            Kind::Copy => w!("Copy with Fast Copy"),
            Kind::Move if fr => w!("Déplacer avec Fast Copy"),
            Kind::Move => w!("Move with Fast Copy"),
            Kind::PasteInto | Kind::PasteHere if fr => w!("Coller ici avec Fast Copy"),
            Kind::PasteInto | Kind::PasteHere => w!("Paste here with Fast Copy"),
        }
    }

    fn canonical(self) -> GUID {
        // Stable, distinct per command (derived from the items CLSID).
        let base = 0xfd3e33d7_c9c3_48ba_beb5_bbe21dce0000u128;
        GUID::from_u128(
            base + match self {
                Kind::RootItems => 1,
                Kind::RootBackground => 2,
                Kind::Copy => 3,
                Kind::Move => 4,
                Kind::PasteInto => 5,
                Kind::PasteHere => 6,
            },
        )
    }

    fn verb(self) -> Option<&'static str> {
        match self {
            Kind::Copy => Some("copy"),
            Kind::Move => Some("move"),
            Kind::PasteInto | Kind::PasteHere => Some("paste"),
            Kind::RootItems | Kind::RootBackground => None,
        }
    }

    fn subcommands(self) -> &'static [Kind] {
        match self {
            Kind::RootItems => &[Kind::Copy, Kind::Move, Kind::PasteInto],
            Kind::RootBackground => &[Kind::PasteHere],
            _ => &[],
        }
    }
}

#[implement(IExplorerCommand)]
struct ExplorerCommand {
    kind: Kind,
    _live: LiveGuard,
}

impl ExplorerCommand {
    fn create(kind: Kind) -> IExplorerCommand {
        ExplorerCommand {
            kind,
            _live: LiveGuard::new(),
        }
        .into()
    }
}

/// Number of items and their AND-ed attributes (restricted to `mask`), or `None` if the
/// array is missing/empty/unreadable.
fn selection_info(items: Option<&IShellItemArray>, mask: SFGAO_FLAGS) -> Option<(u32, u32)> {
    let items = items?;
    // SAFETY: valid COM interface pointer supplied by Explorer.
    let count = unsafe { items.GetCount() }.ok()?;
    if count == 0 {
        return None;
    }
    // Returns S_FALSE (still Ok) when not all items have every bit: we read the value.
    let attrs = unsafe { items.GetAttributes(SIATTRIBFLAGS_AND, mask) }.ok()?;
    Some((count, attrs.0))
}

/// Is there anything to paste? Files on the system clipboard (Ctrl+C/Ctrl+X in Explorer)
/// or Fast Copy's own clipboard file (`%LOCALAPPDATA%\FastCopy\clipboard.json`).
fn has_paste_source() -> bool {
    // SAFETY: plain query, does not open the clipboard.
    if unsafe { IsClipboardFormatAvailable(CF_HDROP.0 as u32) }.is_ok() {
        return true;
    }
    match std::env::var_os("LOCALAPPDATA") {
        Some(dir) => {
            let mut p = std::path::PathBuf::from(dir);
            p.push("FastCopy");
            p.push("clipboard.json");
            p.is_file()
        }
        None => false,
    }
}

impl ExplorerCommand {
    fn state(&self, items: Option<&IShellItemArray>) -> u32 {
        let fs = SFGAO_FILESYSTEM.0;
        let mask = SFGAO_FLAGS(SFGAO_FILESYSTEM.0 | SFGAO_FOLDER.0 | SFGAO_STREAM.0);
        let Some((count, attrs)) = selection_info(items, mask) else {
            return ECS_HIDDEN.0 as u32;
        };
        // Only real file-system items (not "This PC", libraries roots, zip contents…).
        if attrs & fs == 0 {
            return ECS_HIDDEN.0 as u32;
        }
        let is_plain_folder =
            count == 1 && attrs & SFGAO_FOLDER.0 != 0 && attrs & SFGAO_STREAM.0 == 0;
        // Nothing to paste => hide the entry (it sits in the top-level menu: no clutter).
        let paste_state = || {
            if has_paste_source() {
                ECS_ENABLED.0 as u32
            } else {
                ECS_HIDDEN.0 as u32
            }
        };
        match self.kind {
            Kind::RootItems | Kind::Copy | Kind::Move => ECS_ENABLED.0 as u32,
            Kind::PasteInto if is_plain_folder => paste_state(),
            Kind::PasteInto => ECS_HIDDEN.0 as u32,
            // Background: nothing to paste => hide the whole "Fast Copy" entry (no clutter).
            Kind::RootBackground if has_paste_source() => ECS_ENABLED.0 as u32,
            Kind::RootBackground => ECS_HIDDEN.0 as u32,
            Kind::PasteHere => paste_state(),
        }
    }

    fn invoke(&self, items: Option<&IShellItemArray>) -> Result<(), HRESULT> {
        let verb = self.kind.verb().ok_or(E_NOTIMPL)?;
        let items = items.ok_or(E_INVALIDARG)?;
        let paths = collect_paths(items, if verb == "paste" { 1 } else { u32::MAX })?;
        if paths.is_empty() {
            return Err(E_INVALIDARG);
        }
        let exe = exe_path().ok_or(E_FAIL)?;
        let (mut cmdline, listfile) = build_command_line(exe, verb, &paths)?;
        let r = launch(exe, &mut cmdline);
        if r.is_err() {
            if let Some(f) = listfile {
                let _ = std::fs::remove_file(f);
            }
        }
        r
    }
}

impl IExplorerCommand_Impl for ExplorerCommand_Impl {
    fn GetTitle(&self, _items: Option<&IShellItemArray>) -> windows::core::Result<PWSTR> {
        // SAFETY: static NUL-terminated literal; SHStrDupW allocates with CoTaskMemAlloc.
        unsafe { SHStrDupW(self.kind.title()) }
    }

    fn GetIcon(&self, _items: Option<&IShellItemArray>) -> windows::core::Result<PWSTR> {
        let icon = icon_location().ok_or_else(|| windows::core::Error::from(E_NOTIMPL))?;
        // SAFETY: `icon` is NUL-terminated.
        unsafe { SHStrDupW(PCWSTR(icon.as_ptr())) }
    }

    fn GetToolTip(&self, _items: Option<&IShellItemArray>) -> windows::core::Result<PWSTR> {
        Err(E_NOTIMPL.into())
    }

    fn GetCanonicalName(&self) -> windows::core::Result<GUID> {
        Ok(self.kind.canonical())
    }

    fn GetState(
        &self,
        items: Option<&IShellItemArray>,
        _ok_to_be_slow: BOOL,
    ) -> windows::core::Result<u32> {
        Ok(self.state(items))
    }

    fn Invoke(
        &self,
        items: Option<&IShellItemArray>,
        _pbc: Option<&IBindCtx>,
    ) -> windows::core::Result<()> {
        // Every failure is reported as E_FAIL: never let anything escape to Explorer.
        self.invoke(items).map_err(|_| E_FAIL.into())
    }

    fn GetFlags(&self) -> windows::core::Result<u32> {
        Ok(match self.kind {
            Kind::RootItems | Kind::RootBackground => ECF_HASSUBCOMMANDS.0 as u32,
            _ => ECF_DEFAULT.0 as u32,
        })
    }

    fn EnumSubCommands(&self) -> windows::core::Result<IEnumExplorerCommand> {
        let subs = self.kind.subcommands();
        if subs.is_empty() {
            return Err(E_NOTIMPL.into());
        }
        Ok(SubCommands {
            subs,
            pos: AtomicUsize::new(0),
            _live: LiveGuard::new(),
        }
        .into())
    }
}

// ---------------------------------------------------------------------------------------
// Sub-command enumerator
// ---------------------------------------------------------------------------------------

#[implement(IEnumExplorerCommand)]
struct SubCommands {
    subs: &'static [Kind],
    pos: AtomicUsize,
    _live: LiveGuard,
}

impl IEnumExplorerCommand_Impl for SubCommands_Impl {
    fn Next(&self, celt: u32, out: *mut Option<IExplorerCommand>, fetched: *mut u32) -> HRESULT {
        if out.is_null() || (celt > 1 && fetched.is_null()) {
            return E_POINTER;
        }
        let mut n: u32 = 0;
        while n < celt {
            let pos = self.pos.load(Ordering::SeqCst);
            let Some(&kind) = self.subs.get(pos) else {
                break;
            };
            self.pos.store(pos + 1, Ordering::SeqCst);
            // SAFETY: caller provides room for `celt` entries; the slots are uninitialized
            // out-params, so they are written without dropping previous contents.
            unsafe {
                out.add(n as usize)
                    .write(Some(ExplorerCommand::create(kind)))
            };
            n += 1;
        }
        if !fetched.is_null() {
            // SAFETY: checked non-null.
            unsafe { *fetched = n };
        }
        if n == celt {
            S_OK
        } else {
            S_FALSE
        }
    }

    fn Skip(&self, celt: u32) -> windows::core::Result<()> {
        let pos = self
            .pos
            .load(Ordering::SeqCst)
            .saturating_add(celt as usize);
        self.pos.store(pos.min(self.subs.len()), Ordering::SeqCst);
        if pos <= self.subs.len() {
            Ok(())
        } else {
            Err(S_FALSE.into())
        }
    }

    fn Reset(&self) -> windows::core::Result<()> {
        self.pos.store(0, Ordering::SeqCst);
        Ok(())
    }

    fn Clone(&self) -> windows::core::Result<IEnumExplorerCommand> {
        Ok(SubCommands {
            subs: self.subs,
            pos: AtomicUsize::new(self.pos.load(Ordering::SeqCst)),
            _live: LiveGuard::new(),
        }
        .into())
    }
}

// ---------------------------------------------------------------------------------------
// Class factory + DLL exports
// ---------------------------------------------------------------------------------------

#[implement(IClassFactory)]
struct ClassFactory {
    root: Kind,
    _live: LiveGuard,
}

impl IClassFactory_Impl for ClassFactory_Impl {
    fn CreateInstance(
        &self,
        outer: Option<&windows::core::IUnknown>,
        riid: *const GUID,
        ppv: *mut *mut c_void,
    ) -> windows::core::Result<()> {
        if ppv.is_null() || riid.is_null() {
            return Err(E_POINTER.into());
        }
        // SAFETY: checked non-null.
        unsafe { *ppv = core::ptr::null_mut() };
        if outer.is_some() {
            return Err(CLASS_E_NOAGGREGATION.into());
        }
        let cmd = ExplorerCommand::create(self.root);
        // SAFETY: riid/ppv valid (checked above).
        unsafe { cmd.query(riid, ppv) }.ok()
    }

    fn LockServer(&self, lock: BOOL) -> windows::core::Result<()> {
        if lock.as_bool() {
            LIVE.fetch_add(1, Ordering::SeqCst);
        } else {
            LIVE.fetch_sub(1, Ordering::SeqCst);
        }
        Ok(())
    }
}

/// Standard COM entry point: returns the class factory for one of our CLSIDs.
///
/// # Safety
/// Called by COM with valid (or null, which is rejected) pointers.
#[no_mangle]
pub unsafe extern "system" fn DllGetClassObject(
    rclsid: *const GUID,
    riid: *const GUID,
    ppv: *mut *mut c_void,
) -> HRESULT {
    if ppv.is_null() {
        return E_POINTER;
    }
    *ppv = core::ptr::null_mut();
    if rclsid.is_null() || riid.is_null() {
        return E_INVALIDARG;
    }
    let c = *rclsid;
    let root = if c == CLSID_FASTCOPY_ITEMS {
        Kind::RootItems
    } else if c == CLSID_FASTCOPY_BACKGROUND {
        Kind::RootBackground
    } else if c == CLSID_FASTCOPY_COPY {
        Kind::Copy
    } else if c == CLSID_FASTCOPY_MOVE {
        Kind::Move
    } else if c == CLSID_FASTCOPY_PASTE_INTO {
        Kind::PasteInto
    } else if c == CLSID_FASTCOPY_PASTE_HERE {
        Kind::PasteHere
    } else {
        return CLASS_E_CLASSNOTAVAILABLE;
    };
    let factory: IClassFactory = ClassFactory {
        root,
        _live: LiveGuard::new(),
    }
    .into();
    factory.query(riid, ppv)
}

/// Standard COM entry point: the DLL may be unloaded when no object/lock is alive.
#[no_mangle]
pub extern "system" fn DllCanUnloadNow() -> HRESULT {
    if LIVE.load(Ordering::SeqCst) <= 0 {
        S_OK
    } else {
        S_FALSE
    }
}

// ---------------------------------------------------------------------------------------
// Paths, command line, process launch
// ---------------------------------------------------------------------------------------

/// Full path of this DLL (UTF-16, no NUL).
fn dll_path() -> Option<Vec<u16>> {
    static ANCHOR: u8 = 0;
    let mut module = HMODULE::default();
    // SAFETY: FROM_ADDRESS with the address of a static inside this module; UNCHANGED_REFCOUNT
    // so nothing has to be released.
    unsafe {
        GetModuleHandleExW(
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
            PCWSTR(&ANCHOR as *const u8 as *const u16),
            &mut module,
        )
    }
    .ok()?;
    let mut buf = vec![0u16; 1024];
    loop {
        // SAFETY: valid module handle and buffer.
        let n = unsafe { GetModuleFileNameW(module, &mut buf) } as usize;
        if n == 0 {
            return None;
        }
        if n < buf.len() {
            buf.truncate(n);
            return Some(buf);
        }
        if buf.len() >= 32768 {
            return None;
        }
        buf.resize(buf.len() * 2, 0);
    }
}

/// `<dir of this DLL>\fast-copy.exe`, NUL-terminated, computed once.
fn exe_path_nul() -> Option<&'static [u16]> {
    static EXE: OnceLock<Option<Vec<u16>>> = OnceLock::new();
    EXE.get_or_init(|| {
        let mut p = dll_path()?;
        let sep = p
            .iter()
            .rposition(|&c| c == b'\\' as u16 || c == b'/' as u16)?;
        p.truncate(sep + 1);
        p.extend("fast-copy.exe".encode_utf16());
        p.push(0);
        Some(p)
    })
    .as_deref()
}

/// Exe path without the trailing NUL.
fn exe_path() -> Option<&'static [u16]> {
    exe_path_nul().and_then(|p| p.split_last().map(|(_, rest)| rest))
}

/// `"<exe>",0` NUL-terminated (Explorer icon location syntax).
fn icon_location() -> Option<&'static [u16]> {
    static ICON: OnceLock<Option<Vec<u16>>> = OnceLock::new();
    ICON.get_or_init(|| {
        let exe = exe_path()?;
        let mut v = Vec::with_capacity(exe.len() + 4);
        v.extend_from_slice(exe);
        v.extend(",0".encode_utf16());
        v.push(0);
        Some(v)
    })
    .as_deref()
}

/// File-system paths of the items (at most `max`). Items without a file-system path are
/// skipped.
fn collect_paths(items: &IShellItemArray, max: u32) -> Result<Vec<Vec<u16>>, HRESULT> {
    // SAFETY: valid COM interface.
    let count = unsafe { items.GetCount() }.map_err(|e| e.code())?.min(max);
    let mut out = Vec::with_capacity(count as usize);
    for i in 0..count {
        // SAFETY: index < count.
        let Ok(item) = (unsafe { items.GetItemAt(i) }) else {
            continue;
        };
        let Ok(p) = (unsafe { item.GetDisplayName(SIGDN_FILESYSPATH) }) else {
            continue;
        };
        if p.is_null() {
            continue;
        }
        // SAFETY: NUL-terminated string allocated by the shell; freed right after copying.
        let wide = unsafe { p.as_wide() }.to_vec();
        unsafe { CoTaskMemFree(Some(p.0 as *const c_void)) };
        if !wide.is_empty() {
            out.push(wide);
        }
    }
    Ok(out)
}

/// Appends `arg` quoted per the `CommandLineToArgvW` / MSVC CRT rules (backslashes are
/// doubled only when they precede a quote or the closing quote, e.g. `C:\` → `"C:\\"`).
fn append_quoted(out: &mut Vec<u16>, arg: &[u16]) {
    const BS: u16 = b'\\' as u16;
    const QT: u16 = b'"' as u16;
    out.push(QT);
    let mut backslashes = 0usize;
    for &c in arg {
        if c == BS {
            backslashes += 1;
        } else {
            if c == QT {
                for _ in 0..=backslashes {
                    out.push(BS);
                }
            }
            backslashes = 0;
        }
        out.push(c);
    }
    for _ in 0..backslashes {
        out.push(BS);
    }
    out.push(QT);
}

/// Builds the NUL-terminated command line. Returns the list file path if one was written.
fn build_command_line(
    exe: &[u16],
    verb: &str,
    paths: &[Vec<u16>],
) -> Result<(Vec<u16>, Option<std::path::PathBuf>), HRESULT> {
    let mut cmd: Vec<u16> =
        Vec::with_capacity(exe.len() + 32 + paths.iter().map(|p| p.len() + 3).sum::<usize>());
    append_quoted(&mut cmd, exe);
    cmd.extend(" --shell ".encode_utf16());
    cmd.extend(verb.encode_utf16());
    let base_len = cmd.len();
    let mut too_long = paths.len() > MAX_INLINE_ITEMS as usize;
    if !too_long {
        for p in paths {
            cmd.push(b' ' as u16);
            append_quoted(&mut cmd, p);
            if cmd.len() > MAX_CMDLINE {
                too_long = true;
                break;
            }
        }
    }
    let mut listfile = None;
    if too_long {
        cmd.truncate(base_len);
        let file = write_list_file(paths).map_err(|_| E_FAIL)?;
        let mut arg: Vec<u16> = vec![b'@' as u16];
        arg.extend(file.as_os_str().encode_wide_vec());
        cmd.push(b' ' as u16);
        append_quoted(&mut cmd, &arg);
        listfile = Some(file);
    }
    cmd.push(0);
    Ok((cmd, listfile))
}

trait EncodeWideVec {
    fn encode_wide_vec(&self) -> Vec<u16>;
}
impl EncodeWideVec for std::ffi::OsStr {
    fn encode_wide_vec(&self) -> Vec<u16> {
        use std::os::windows::ffi::OsStrExt;
        self.encode_wide().collect()
    }
}

/// Writes `%TEMP%\fastcopy-shell-<pid>-<n>.lst`: UTF-8, one path per line (CRLF-free `\n`).
fn write_list_file(paths: &[Vec<u16>]) -> std::io::Result<std::path::PathBuf> {
    static SEQ: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir();
    let pid = std::process::id();
    let mut text = String::with_capacity(paths.iter().map(|p| p.len() + 1).sum());
    for p in paths {
        for c in char::decode_utf16(p.iter().copied()) {
            text.push(c.unwrap_or(char::REPLACEMENT_CHARACTER));
        }
        text.push('\n');
    }
    for _ in 0..16 {
        let n = SEQ.fetch_add(1, Ordering::SeqCst);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        let path = dir.join(format!("fastcopy-shell-{pid}-{nanos:08x}-{n}.lst"));
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(mut f) => {
                if let Err(e) = f.write_all(text.as_bytes()) {
                    drop(f);
                    let _ = std::fs::remove_file(&path);
                    return Err(e);
                }
                return Ok(path);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::from(std::io::ErrorKind::AlreadyExists))
}

/// `CreateProcessW` with an explicit application name (no search path), no handle
/// inheritance, and foreground rights handed to the child.
fn launch(exe: &[u16], cmdline: &mut [u16]) -> Result<(), HRESULT> {
    let exe_nul = exe_path_nul().ok_or(E_FAIL)?;
    if exe_nul.len() != exe.len() + 1 {
        return Err(E_FAIL);
    }
    let si = STARTUPINFOW {
        cb: core::mem::size_of::<STARTUPINFOW>() as u32,
        ..Default::default()
    };
    let mut pi = PROCESS_INFORMATION::default();
    // SAFETY: NUL-terminated application name and mutable, NUL-terminated command line.
    unsafe {
        CreateProcessW(
            PCWSTR(exe_nul.as_ptr()),
            PWSTR(cmdline.as_mut_ptr()),
            None,
            None,
            false,
            CREATE_DEFAULT_ERROR_MODE,
            None,
            PCWSTR::null(),
            &si,
            &mut pi,
        )
    }
    .map_err(|e| e.code())?;
    // SAFETY: handles returned by CreateProcessW, closed exactly once.
    unsafe {
        let _ = AllowSetForegroundWindow(pi.dwProcessId);
        let _ = windows::Win32::Foundation::CloseHandle(pi.hThread);
        let _ = windows::Win32::Foundation::CloseHandle(pi.hProcess);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }
    fn q(s: &str) -> String {
        let mut v = Vec::new();
        append_quoted(&mut v, &w(s));
        String::from_utf16(&v).unwrap()
    }

    #[test]
    fn quoting() {
        assert_eq!(q(r"C:\a b\c.txt"), r#""C:\a b\c.txt""#);
        assert_eq!(q(r"C:\"), r#""C:\\""#);
        assert_eq!(q(r"\\srv\share\dir\"), r#""\\srv\share\dir\\""#);
        assert_eq!(q(r#"a"b"#), r#""a\"b""#);
    }

    #[test]
    fn inline_and_listfile() {
        let exe = w(r"C:\Program Files\Fast Copy\fast-copy.exe");
        let (cmd, lf) = build_command_line(&exe, "copy", &[w(r"C:\x"), w(r"D:\")]).unwrap();
        assert!(lf.is_none());
        let s = String::from_utf16(&cmd[..cmd.len() - 1]).unwrap();
        assert_eq!(
            s,
            r#""C:\Program Files\Fast Copy\fast-copy.exe" --shell copy "C:\x" "D:\\""#
        );

        let many: Vec<Vec<u16>> = (0..25).map(|i| w(&format!(r"C:\dir\é{i}"))).collect();
        let (cmd, lf) = build_command_line(&exe, "move", &many).unwrap();
        let lf = lf.unwrap();
        let s = String::from_utf16(&cmd[..cmd.len() - 1]).unwrap();
        assert!(s.contains(" --shell move \"@"));
        let text = std::fs::read_to_string(&lf).unwrap();
        std::fs::remove_file(&lf).unwrap();
        assert_eq!(text.lines().count(), 25);
        assert_eq!(text.lines().next(), Some(r"C:\dir\é0"));
    }
}
