; Registers the Explorer shell extensions after the app is installed, and
; removes them on uninstall. Referenced from `tauri.conf.json` as
; `bundle.windows.nsis.installerHooks`.
;
; The DLL carries its own registration — `regsvr32` calls its
; `DllRegisterServer`, which writes the class keys, the two `ShellEx` slots
; under `.hep`, and the HKLM preview-handler list. That is why there are no
; `WriteRegStr` lines here: keeping the registry layout in one place, next to
; the code that depends on it, is worth more than doing it in NSIS.
;
; `/s` is silent — an installer must not pop a dialog. Failure is deliberately
; not fatal: a missing thumbnail is not a reason to fail an install.
;
; UNTESTED. Nobody has run a Windows build. See
; ../../hephaestus-explorer/CLAUDE.md.

!macro NSIS_HOOK_POSTINSTALL
  DetailPrint "Registering Explorer shell extensions..."
  nsExec::ExecToLog '"$SYSDIR\regsvr32.exe" /s "$INSTDIR\hephaestus_explorer.dll"'
  Pop $0
  ${If} $0 != 0
    DetailPrint "Shell extension registration failed ($0); thumbnails and the preview pane will be unavailable."
  ${EndIf}
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  DetailPrint "Unregistering Explorer shell extensions..."
  nsExec::ExecToLog '"$SYSDIR\regsvr32.exe" /s /u "$INSTDIR\hephaestus_explorer.dll"'
  Pop $0
!macroend
