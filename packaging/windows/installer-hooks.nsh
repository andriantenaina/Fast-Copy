; Fast Copy - Tauri v2 NSIS installer hooks.
; Referenced from app/src-tauri/tauri.conf.json (or tauri.windows.conf.json):
;   "bundle": { "windows": { "nsis": { "installerHooks": "../../packaging/windows/installer-hooks.nsh" } } }
;
; Post-install:
;   1. "fast-copy.exe --register-shell": per-user registry verbs (classic menu / Windows 10 /
;      "Show more options") + Send To shortcut (fc_core::shell::install). No admin needed.
;   2. If FastCopy.msix and fc_shellext_win.dll were bundled next to the exe: register the
;      sparse package (Windows 11 top-level menu) with $INSTDIR as external location.
;      Failures are ignored (unsigned/untrusted package, Windows 10 < 2004, policy...):
;      the classic menu from step 1 still works.
; Pre-uninstall: reverse order.
;
; Everything runs hidden through nsExec and never aborts the (un)installer.

!include LogicLib.nsh

!define FASTCOPY_SPARSE_NAME "FastCopy.ShellExtension"

!macro NSIS_HOOK_POSTINSTALL
  Push $0 ; preserve the caller's $0
  DetailPrint "Fast Copy: registering Explorer integration..."
  ${If} ${FileExists} "$INSTDIR\fast-copy.exe"
    nsExec::Exec '"$INSTDIR\fast-copy.exe" --register-shell'
    Pop $0
  ${EndIf}

  ${If} ${FileExists} "$INSTDIR\FastCopy.msix"
  ${AndIf} ${FileExists} "$INSTDIR\fc_shellext_win.dll"
    DetailPrint "Fast Copy: registering the Windows 11 context menu package..."
    nsExec::Exec `powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -WindowStyle Hidden -Command "try { Add-AppxPackage -Path '$INSTDIR\FastCopy.msix' -ExternalLocation '$INSTDIR' -ForceUpdateFromAnyVersion -ErrorAction Stop; exit 0 } catch { exit 1 }"`
    Pop $0
    ${If} $0 != 0
      DetailPrint "Fast Copy: sparse package not registered (unsigned or untrusted?) - the classic context menu is still available."
    ${EndIf}
  ${EndIf}
  Pop $0
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  Push $0
  DetailPrint "Fast Copy: removing Explorer integration..."
  nsExec::Exec `powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -WindowStyle Hidden -Command "Get-AppxPackage -Name '${FASTCOPY_SPARSE_NAME}' | Remove-AppxPackage -ErrorAction SilentlyContinue; exit 0"`
  Pop $0

  ${If} ${FileExists} "$INSTDIR\fast-copy.exe"
    nsExec::Exec '"$INSTDIR\fast-copy.exe" --unregister-shell'
    Pop $0
  ${EndIf}
  ; Explorer may still have fc_shellext_win.dll loaded: schedule its deletion at reboot if the
  ; regular uninstall step cannot remove it.
  Delete /REBOOTOK "$INSTDIR\fc_shellext_win.dll"
!macroend
