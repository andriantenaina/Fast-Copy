# Windows packaging — Explorer integration

Fast Copy integrates with File Explorer at two levels:

| Menu | Provided by | Needs |
|------|-------------|-------|
| Windows 10 menu, Windows 11 "Show more options", Send To | per-user registry verbs written by `fast-copy.exe --register-shell` (`fc_core::shell::install`) | nothing (no admin, no signature) |
| Windows 11 top-level menu ("Fast Copy ▸ Copier / Déplacer / Coller ici") | `fc_shellext_win.dll` (COM `IExplorerCommand`) + sparse package `FastCopy.msix` | a **signed** `FastCopy.msix` whose signature is trusted |

## Files

| File | Role |
|------|------|
| `sparse/AppxManifest.xml` | sparse package manifest (`uap10:AllowExternalContent`, `desktop4/desktop5` File Explorer verbs, `com:SurrogateServer`) |
| `sparse/Assets/*.png` | fallback logos (placeholder "»" icon). `build-sparse.ps1` uses `app/src-tauri/icons/{Square44x44Logo,Square150x150Logo,StoreLogo}.png` (Tauri-generated) when they exist |
| `tauri.windows.bundle.json` | reference copy of the Windows bundle config (DLL, `fcopy.exe`, MSIX, NSIS hooks); `app/src-tauri/tauri.windows.conf.json` declares everything but the MSIX, which the release workflow adds with `tauri build --config` |
| `build-sparse.ps1` | stage manifest → `MakeAppx pack /nv` → `SignTool sign` (PFX, or `-SelfSigned` dev cert) |
| `register-sparse.ps1` / `unregister-sparse.ps1` | `Add-AppxPackage -Path FastCopy.msix -ExternalLocation <installdir>` / `Remove-AppxPackage` (dev / manual use) |
| `installer-hooks.nsh` | Tauri v2 NSIS hooks (`NSIS_HOOK_POSTINSTALL`, `NSIS_HOOK_PREUNINSTALL`) |

## COM classes (crate `crates/fc-shellext-win`)

| CLSID | Explorer item types | Sub-menu |
|-------|---------------------|----------|
| `{FD3E33D7-C9C3-48BA-BEB5-BBE21DCEDFC1}` | `*`, `Directory` | Copier / Déplacer avec Fast Copy (+ Coller ici on a single folder) |
| `{2F9A8E37-3C38-4E7A-8B4C-61AE88A3E347}` | `Directory\Background` | Coller ici avec Fast Copy (whole entry hidden when nothing to paste) |

The DLL launches `fast-copy.exe` **from its own folder**:

```
fast-copy.exe --shell copy  "<path1>" "<path2>" ...
fast-copy.exe --shell move  "<path1>" ...
fast-copy.exe --shell paste "<target folder>"
fast-copy.exe --shell copy|move @<listfile>      (> 20 items or command line > 8000 chars)
```

`<listfile>` = `%TEMP%\fastcopy-shell-<pid>-<n>.lst`, UTF-8 without BOM, one absolute path per
line (`\n`). **The GUI must delete it after reading it.** Labels are French when the user UI
language is French, English otherwise. "Paste" is enabled only when there are files on the
system clipboard (CF_HDROP) or `%LOCALAPPDATA%\FastCopy\clipboard.json` exists.

## What the GUI (`app/src-tauri`) must do

1. Handle the CLI flags `--register-shell` (→ `fc_core::shell::install(&current_exe, &fc_core::shell::system_lang())`)
   and `--unregister-shell` (→ `fc_core::shell::uninstall()`), exiting immediately without
   opening a window; `--shell <verb> <paths|@listfile>` as above.
2. Bundle the DLL and the package next to `fast-copy.exe` and use the hooks. Put this in
   **`app/src-tauri/tauri.windows.conf.json`** (Windows-only config, merged by Tauri on
   Windows; paths are relative to `app/src-tauri`):

```json
{
  "bundle": {
    "resources": {
      "../../target/release/fc_shellext_win.dll": "fc_shellext_win.dll",
      "../../packaging/windows/out/FastCopy.msix": "FastCopy.msix"
    },
    "windows": {
      "nsis": {
        "installerHooks": "../../packaging/windows/installer-hooks.nsh"
      }
    }
  }
}
```

   (When building with `--target <triple>`, the DLL is in `../../target/<triple>/release/`;
   the release workflow copies it to `target/release/` before bundling so the path above
   stays valid. On Windows, Tauri's resource dir is the install dir, i.e. next to the exe.)

3. Before `tauri build` on Windows:

```powershell
cargo build -p fc-shellext-win --release
powershell -File packaging/windows/build-sparse.ps1 -CertPath <pfx> -CertPassword <pwd> -Publisher "<cert subject>"
#   or, for a local dev build:  -SelfSigned   (see below)
```

The MSI (WiX) bundle gets the same resources but no hooks; MSI users get the classic menu
the first time the app runs if the GUI calls `fc_core::shell::install` when
`!fc_core::shell::is_installed()` (recommended), and can register the sparse package with
`register-sparse.ps1`.

## Dev loop with a self-signed certificate

```powershell
cargo build -p fc-shellext-win --release
.\packaging\windows\build-sparse.ps1 -SelfSigned -AppDir <folder with fast-copy.exe + fc_shellext_win.dll>
# once, as admin: trust the dev cert for package installation
Import-Certificate -FilePath .\packaging\windows\out\FastCopy-dev.cer -CertStoreLocation Cert:\LocalMachine\TrustedPeople
.\packaging\windows\register-sparse.ps1 -InstallDir <same folder> -RestartExplorer
# ... test ...
.\packaging\windows\unregister-sparse.ps1
```

`-SelfSigned` creates (or reuses) a code-signing cert `Subject = -Publisher` (default
`CN=Fast Copy`) in `Cert:\CurrentUser\My` (Windows also copies it to `Cert:\CurrentUser\CA`),
exports `out\FastCopy-dev.pfx` (password `fastcopy-dev` unless `-CertPassword`) and
`out\FastCopy-dev.cer`. Add `-RemoveCertFromStore` to delete it from both stores after signing.
Clean up later with `Get-ChildItem Cert:\CurrentUser\My, Cert:\CurrentUser\CA, Cert:\LocalMachine\TrustedPeople | ? Subject -eq 'CN=Fast Copy' | Remove-Item`.

**The manifest `Publisher` must be identical to the certificate Subject** (pass
`-Publisher "CN=..., O=..., C=..."` exactly as in the cert for production builds).

## Sanity check of the DLL (no registration needed)

```powershell
cargo build -p fc-shellext-win --release
cargo run -p fc-shellext-win --example load_dll --release
```

Loads the DLL with `LoadLibraryW`, goes through `DllGetClassObject → IClassFactory →
IExplorerCommand`, enumerates the sub-menu, checks states, invokes copy (inline paths) and
move (25 files → `@listfile`) against a dummy `fast-copy.exe`, and checks `DllCanUnloadNow`.

## Debugging the modern menu

* `Get-AppxPackage FastCopy.ShellExtension` must list the package with the right
  `InstallLocation` (= external location).
* Explorer caches menus: restart it (`register-sparse.ps1 -RestartExplorer`).
* Errors at registration: `Get-AppxLog -ActivityId <id>` (the id is printed by Add-AppxPackage).
* Arm64 Windows: build the DLL for `aarch64-pc-windows-msvc` and pack with `-Arch arm64`.
