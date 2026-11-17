//! Sanity harness for the shell extension DLL (no Explorer, no registration needed).
//!
//! 1. copies `fc_shellext_win.dll` and this very executable (renamed `fast-copy.exe`) into a
//!    temp folder, so the DLL finds "its" `fast-copy.exe` next to it;
//! 2. `LoadLibraryW` + `DllGetClassObject` for both CLSIDs -> `IClassFactory` ->
//!    `IExplorerCommand`; prints title / icon / flags / sub-commands and their states;
//! 3. invokes "copy" on 2 files (inline arguments) and "move" on 25 files (`@listfile`),
//!    the dummy `fast-copy.exe` (this program run with `--shell …`) records what it got;
//! 4. checks `DllCanUnloadNow() == S_OK` once everything is released.
//!
//! Run: `cargo build -p fc-shellext-win --release && cargo run -p fc-shellext-win --example load_dll --release`
//! (optionally pass the DLL path as first argument). Exit code 0 = all checks passed.

#[cfg(not(windows))]
fn main() {
    eprintln!("Windows only");
}

#[cfg(windows)]
fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("--shell") {
        dummy_fast_copy(&args[2..]);
        return;
    }
    match win::run(args.get(1).cloned()) {
        Ok(()) => println!("\nALL CHECKS PASSED"),
        Err(e) => {
            eprintln!("\nFAILED: {e}");
            std::process::exit(1);
        }
    }
}

/// Behaves as `fast-copy.exe --shell <verb> <args…>`: records its arguments (and the content of
/// an `@listfile`, which it then deletes, like the GUI does) into `$FC_SHELLEXT_TEST_OUT`.
#[cfg(windows)]
fn dummy_fast_copy(args: &[String]) {
    let Some(out) = std::env::var_os("FC_SHELLEXT_TEST_OUT") else {
        return;
    };
    let mut text = String::new();
    for a in args {
        text.push_str("ARG ");
        text.push_str(a);
        text.push('\n');
        if let Some(list) = a.strip_prefix('@') {
            if let Ok(content) = std::fs::read_to_string(list) {
                for line in content.lines() {
                    text.push_str("LIST ");
                    text.push_str(line);
                    text.push('\n');
                }
                let _ = std::fs::remove_file(list);
            }
        }
    }
    let tmp = std::path::PathBuf::from(&out).with_extension("tmp");
    let _ = std::fs::write(&tmp, text);
    let _ = std::fs::rename(&tmp, &out);
}

#[cfg(windows)]
mod win {
    use std::ffi::c_void;
    use std::os::windows::ffi::OsStrExt;
    use std::path::{Path, PathBuf};
    use std::time::{Duration, Instant};

    use windows::core::{Interface, GUID, HRESULT, PCSTR, PCWSTR};
    use windows::Win32::Foundation::{FreeLibrary, S_OK};
    use windows::Win32::System::Com::{
        CoInitializeEx, CoTaskMemFree, IClassFactory, COINIT_APARTMENTTHREADED,
    };
    use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
    use windows::Win32::UI::Shell::Common::ITEMIDLIST;
    use windows::Win32::UI::Shell::{
        IExplorerCommand, ILCreateFromPathW, ILFree, IShellItemArray,
        SHCreateShellItemArrayFromIDLists,
    };

    const CLSID_ITEMS: GUID = GUID::from_u128(0xfd3e33d7_c9c3_48ba_beb5_bbe21dcedfc1);
    const CLSID_BACKGROUND: GUID = GUID::from_u128(0x2f9a8e37_3c38_4e7a_8b4c_61ae88a3e347);
    const CANON_COPY: GUID = GUID::from_u128(0xfd3e33d7_c9c3_48ba_beb5_bbe21dce0003);
    const CANON_MOVE: GUID = GUID::from_u128(0xfd3e33d7_c9c3_48ba_beb5_bbe21dce0004);

    type GetClassObjectFn =
        unsafe extern "system" fn(*const GUID, *const GUID, *mut *mut c_void) -> HRESULT;
    type CanUnloadFn = unsafe extern "system" fn() -> HRESULT;

    fn wide(p: &Path) -> Vec<u16> {
        p.as_os_str().encode_wide().chain(Some(0)).collect()
    }

    fn take_pwstr(p: windows::core::PWSTR) -> String {
        // SAFETY: CoTaskMem string returned by the DLL.
        let s = unsafe { p.to_string() }.unwrap_or_default();
        unsafe { CoTaskMemFree(Some(p.0 as *const c_void)) };
        s
    }

    fn check(cond: bool, what: &str) -> Result<(), String> {
        if cond {
            println!("  ok: {what}");
            Ok(())
        } else {
            Err(what.to_string())
        }
    }

    fn shell_array(paths: &[PathBuf]) -> Result<IShellItemArray, String> {
        let pidls: Vec<*mut ITEMIDLIST> = paths
            .iter()
            .map(|p| unsafe { ILCreateFromPathW(PCWSTR(wide(p).as_ptr())) })
            .collect();
        if pidls.iter().any(|p| p.is_null()) {
            return Err("ILCreateFromPathW failed".into());
        }
        let c: Vec<*const ITEMIDLIST> = pidls.iter().map(|p| *p as *const _).collect();
        let arr = unsafe { SHCreateShellItemArrayFromIDLists(&c) }.map_err(|e| e.to_string());
        for p in pidls {
            unsafe { ILFree(Some(p)) };
        }
        arr
    }

    fn describe(
        cmd: &IExplorerCommand,
        items: Option<&IShellItemArray>,
        indent: &str,
    ) -> windows::core::Result<()> {
        unsafe {
            let title = take_pwstr(cmd.GetTitle(items)?);
            let flags = cmd.GetFlags()?;
            let state = cmd.GetState(items, true)?;
            println!(
                "{indent}title={title:?} flags={flags} state={state} canonical={:?}",
                cmd.GetCanonicalName()?
            );
        }
        Ok(())
    }

    fn subcommands(root: &IExplorerCommand) -> windows::core::Result<Vec<IExplorerCommand>> {
        let e = unsafe { root.EnumSubCommands()? };
        let mut out = Vec::new();
        loop {
            let mut slot = [None];
            let mut fetched = 0u32;
            let hr = unsafe { e.Next(&mut slot, Some(&mut fetched)) };
            if hr != S_OK || fetched == 0 {
                break;
            }
            if let Some(c) = slot[0].take() {
                out.push(c);
            }
        }
        Ok(out)
    }

    fn wait_output(out: &Path) -> Result<String, String> {
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(15) {
            if let Ok(s) = std::fs::read_to_string(out) {
                let _ = std::fs::remove_file(out);
                return Ok(s);
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        Err("dummy fast-copy.exe produced no output (not launched?)".into())
    }

    pub fn run(dll_arg: Option<String>) -> Result<(), String> {
        let me = std::env::current_exe().map_err(|e| e.to_string())?;
        let src_dll = match dll_arg {
            Some(p) => PathBuf::from(p),
            // target/<profile>/examples/load_dll.exe -> target/<profile>/fc_shellext_win.dll
            None => me
                .parent()
                .and_then(Path::parent)
                .ok_or("no parent")?
                .join("fc_shellext_win.dll"),
        };
        let dir = std::env::temp_dir().join(format!("fc-shellext-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("data")).map_err(|e| e.to_string())?;
        let dll = dir.join("fc_shellext_win.dll");
        std::fs::copy(&src_dll, &dll).map_err(|e| format!("copy {}: {e}", src_dll.display()))?;
        std::fs::copy(&me, dir.join("fast-copy.exe")).map_err(|e| e.to_string())?;
        let out = dir.join("out.txt");
        std::env::set_var("FC_SHELLEXT_TEST_OUT", &out);
        println!("DLL: {}\nTest dir: {}", src_dll.display(), dir.display());

        let mut files = Vec::new();
        for i in 0..25 {
            let f = dir.join("data").join(format!("fichier é {i}.txt"));
            std::fs::write(&f, b"x").map_err(|e| e.to_string())?;
            files.push(f);
        }

        let result = unsafe { run_inner(&dll, &dir, &files, &out) };
        let _ = std::fs::remove_dir_all(&dir);
        result
    }

    unsafe fn run_inner(
        dll: &Path,
        dir: &Path,
        files: &[PathBuf],
        out: &Path,
    ) -> Result<(), String> {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let module =
            LoadLibraryW(PCWSTR(wide(dll).as_ptr())).map_err(|e| format!("LoadLibraryW: {e}"))?;
        let gco = GetProcAddress(module, PCSTR(c"DllGetClassObject".as_ptr().cast()))
            .ok_or("DllGetClassObject not exported")?;
        let gco: GetClassObjectFn = std::mem::transmute(gco);
        let can_unload = GetProcAddress(module, PCSTR(c"DllCanUnloadNow".as_ptr().cast()))
            .ok_or("DllCanUnloadNow not exported")?;
        let can_unload: CanUnloadFn = std::mem::transmute(can_unload);

        {
            // Unknown CLSID must be refused.
            let mut p = std::ptr::null_mut();
            let hr = gco(&GUID::from_u128(1), &IClassFactory::IID, &mut p);
            check(
                hr.is_err() && p.is_null(),
                "unknown CLSID -> CLASS_E_CLASSNOTAVAILABLE",
            )?;

            let make = |clsid: &GUID| -> Result<IExplorerCommand, String> {
                let mut p = std::ptr::null_mut();
                gco(clsid, &IClassFactory::IID, &mut p)
                    .ok()
                    .map_err(|e| format!("DllGetClassObject: {e}"))?;
                let factory = IClassFactory::from_raw(p);
                factory
                    .CreateInstance::<Option<&windows::core::IUnknown>, IExplorerCommand>(None)
                    .map_err(|e| format!("CreateInstance: {e}"))
            };

            let two = shell_array(&files[..2])?;
            let many = shell_array(files)?;
            let folder = shell_array(&[dir.join("data")])?;

            // Flat top-level classes registered by the sparse package.
            println!("\n[flat CLSIDs] one top-level entry each:");
            let flat: [(u128, &str); 4] = [
                (0x5ac697db_2a91_47ae_9df4_83d916bf7025, "Copier avec Fast Copy|Copy with Fast Copy"),
                (0x11ffd5d3_8a78_4a65_a664_449cd515a7a0, "Déplacer avec Fast Copy|Move with Fast Copy"),
                (0x0c57fa8b_4d91_4528_b75d_62e16cceb676, "Coller ici avec Fast Copy|Paste here with Fast Copy"),
                (0x1cc48706_c26d_4b46_b786_28a0942ab45e, "Coller ici avec Fast Copy|Paste here with Fast Copy"),
            ];
            for (id, titles) in flat {
                let cmd = make(&GUID::from_u128(id))?;
                let title = take_pwstr(cmd.GetTitle(None).map_err(|e| e.to_string())?);
                let flags = cmd.GetFlags().map_err(|e| e.to_string())?;
                println!("  {title} flags={flags}");
                check(titles.split('|').any(|t| t == title), "flat title")?;
                check(flags == 0, "flat entry has no sub-menu (ECF_DEFAULT)")?;
            }
            let copy = make(&GUID::from_u128(flat[0].0))?;
            check(copy.GetState(Some(&two), true).map_err(|e| e.to_string())? == 0, "flat copy enabled on 2 files")?;

            println!("\n[items CLSID] root, 2 files selected:");
            let root = make(&CLSID_ITEMS)?;
            describe(&root, Some(&two), "  ").map_err(|e| e.to_string())?;
            let icon = take_pwstr(root.GetIcon(None).map_err(|e| e.to_string())?);
            println!("  icon={icon}");
            check(
                icon.ends_with("fast-copy.exe,0"),
                "icon points to fast-copy.exe next to the DLL",
            )?;
            check(
                root.GetFlags().map_err(|e| e.to_string())? == 1,
                "root has ECF_HASSUBCOMMANDS",
            )?;
            check(
                root.GetState(None, true).map_err(|e| e.to_string())? == 2,
                "root hidden with no selection",
            )?;
            let subs = subcommands(&root).map_err(|e| e.to_string())?;
            check(subs.len() == 3, "3 sub-commands")?;
            for s in &subs {
                describe(s, Some(&two), "    ").map_err(|e| e.to_string())?;
            }
            println!("  sub-command states with 1 folder selected:");
            for s in &subs {
                describe(s, Some(&folder), "    ").map_err(|e| e.to_string())?;
            }

            let copy = subs
                .iter()
                .find(|s| s.GetCanonicalName().ok() == Some(CANON_COPY))
                .ok_or("no copy cmd")?;
            let mv = subs
                .iter()
                .find(|s| s.GetCanonicalName().ok() == Some(CANON_MOVE))
                .ok_or("no move cmd")?;

            println!("\n[invoke copy, 2 files]");
            copy.Invoke(&two, None)
                .map_err(|e| format!("Invoke: {e}"))?;
            let got = wait_output(out)?;
            print!("{got}");
            let lines: Vec<&str> = got.lines().collect();
            check(lines.first() == Some(&"ARG copy"), "verb = copy")?;
            check(
                lines.get(1).map(|l| l.trim_start_matches("ARG "))
                    == Some(files[0].to_str().unwrap_or("")),
                "first path received intact (spaces + accents)",
            )?;
            check(lines.len() == 3, "2 paths inline")?;

            println!("\n[invoke move, 25 files -> @listfile]");
            mv.Invoke(&many, None).map_err(|e| format!("Invoke: {e}"))?;
            let got = wait_output(out)?;
            let lines: Vec<&str> = got.lines().collect();
            for l in lines.iter().take(4) {
                println!("{l}");
            }
            println!("... ({} lines)", lines.len());
            check(lines.first() == Some(&"ARG move"), "verb = move")?;
            check(
                lines.get(1).is_some_and(|l| l.starts_with("ARG @")),
                "@listfile argument",
            )?;
            check(
                lines.iter().filter(|l| l.starts_with("LIST ")).count() == 25,
                "25 paths in list file (UTF-8)",
            )?;
            check(
                lines.get(2).map(|l| l.trim_start_matches("LIST "))
                    == Some(files[0].to_str().unwrap_or("")),
                "list path intact",
            )?;

            println!("\n[background CLSID] root on folder background:");
            let bg = make(&CLSID_BACKGROUND)?;
            describe(&bg, Some(&folder), "  ").map_err(|e| e.to_string())?;
            for s in subcommands(&bg).map_err(|e| e.to_string())? {
                describe(&s, Some(&folder), "    ").map_err(|e| e.to_string())?;
            }
        }
        // Everything released: the DLL must agree to unload.
        check(
            can_unload() == S_OK,
            "DllCanUnloadNow == S_OK after releasing all objects",
        )?;
        let _ = FreeLibrary(module);
        Ok(())
    }
}
