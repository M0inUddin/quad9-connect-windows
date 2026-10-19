Unicode true
RequestExecutionLevel admin
ManifestDPIAware true
SetCompressor /SOLID lzma
!include MUI2.nsh
!include nsDialogs.nsh
!include LogicLib.nsh
!include FileFunc.nsh
!include x64.nsh

!ifndef PAYLOAD
  !error "Build with scripts/Build-WindowsInstaller.ps1"
!endif
Name "Quad9 Connect ${VERSION} (unsigned development build)"
OutFile "${OUTPUT}"
VIProductVersion "0.1.0.0"
VIAddVersionKey /LANG=1033 "ProductName" "Quad9 Connect development installer"
VIAddVersionKey /LANG=1033 "FileDescription" "Unsigned community development build"
VIAddVersionKey /LANG=1033 "FileVersion" "${VERSION}"
VIAddVersionKey /LANG=1033 "LegalCopyright" "Quad9 Connect community contributors"
Icon "..\..\apps\desktop\src-tauri\icons\icon.ico"
ShowInstDetails show
ShowUninstDetails show
Var OperatorSid
Var OperatorList
Var UserCount
Var Index
Var Parameters
Var Dialog

!define MUI_WELCOMEPAGE_TEXT "This unsigned development build installs the community Quad9 Connect app and its privileged service.$\r$\n$\r$\nDNS stays unchanged until you enable it in the app. Upgrades preserve existing DNS management and recovery records.$\r$\n$\r$\nThis project is independent of Quad9."
!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_LICENSE "..\..\LICENSE"
Page custom OperatorPage OperatorLeave
!insertmacro MUI_PAGE_INSTFILES
!define MUI_FINISHPAGE_TEXT "Installation finished. Open Quad9 Connect from the Start menu. A newly enrolled operator must sign out and sign in again before their token includes DNS control rights.$\r$\n$\r$\nThe interface runs without elevation."
!insertmacro MUI_PAGE_FINISH
!define MUI_UNCONFIRMPAGE_TEXT_TOP "Uninstall restores DNS settings still owned by Quad9 Connect before removing the service. If restoration cannot finish, removal stops and retains the recovery mechanism."
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE English

Function .onInit
  ${IfNot} ${RunningX64}
    MessageBox MB_ICONSTOP "This build requires 64-bit Windows 11."
    SetErrorLevel 1
    Quit
  ${EndIf}
  ${DisableX64FSRedirection}
  SetRegView 64
  InitPluginsDir
  SetOutPath "$PLUGINSDIR"
  File "${PAYLOAD}\*"
  File "List-Operators.ps1"
  ${GetParameters} $Parameters
  ${GetOptions} $Parameters "/OPERATORSID=" $OperatorSid
  # Only SID characters may reach a command-line argument, even for silent setup.
  StrCpy $Index 0
  validate_sid:
    StrCpy $0 $OperatorSid 1 $Index
    StrCmp $0 "" validated_sid
    StrCmp $0 "S" sid_character_ok
    StrCmp $0 "-" sid_character_ok
    StrCmp $0 "0" sid_character_ok
    StrCmp $0 "1" sid_character_ok
    StrCmp $0 "2" sid_character_ok
    StrCmp $0 "3" sid_character_ok
    StrCmp $0 "4" sid_character_ok
    StrCmp $0 "5" sid_character_ok
    StrCmp $0 "6" sid_character_ok
    StrCmp $0 "7" sid_character_ok
    StrCmp $0 "8" sid_character_ok
    StrCmp $0 "9" sid_character_ok
    SetErrorLevel 1
    Quit
    sid_character_ok:
      IntOp $Index $Index + 1
      IntCmp $Index 184 validate_sid validate_sid
      SetErrorLevel 1
      Quit
  validated_sid:
FunctionEnd

Function OperatorPage
  nsDialogs::Create 1018
  Pop $Dialog
  ${If} $Dialog == error
    Abort
  ${EndIf}
  ${NSD_CreateLabel} 0 0 100% 52u "Select a local user to control Quad9 Connect. Membership in Quad9ConnectOperators allows changes to machine-wide DNS.$\r$\n$\r$\nFor an upgrade, keep existing operator access."
  Pop $0
  ${NSD_CreateDropList} 0 62u 100% 120u ""
  Pop $OperatorList
  ${NSD_CB_AddString} $OperatorList "Keep existing access (upgrades only)"
  nsExec::ExecToLog '"$SYSDIR\WindowsPowerShell\v1.0\powershell.exe" -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "$PLUGINSDIR\List-Operators.ps1" -OutputPath "$PLUGINSDIR\users.ini"'
  Pop $0
  ${If} $0 != 0
    MessageBox MB_ICONSTOP "Could not list local users. Installation has stopped."
    Abort
  ${EndIf}
  ReadINIStr $UserCount "$PLUGINSDIR\users.ini" Users count
  StrCpy $Index 0
  ${While} $Index < $UserCount
    ReadINIStr $0 "$PLUGINSDIR\users.ini" Users "name$Index"
    ${NSD_CB_AddString} $OperatorList $0
    IntOp $Index $Index + 1
  ${EndWhile}
  SendMessage $OperatorList ${CB_SETCURSEL} 0 0
  nsDialogs::Show
FunctionEnd

Function OperatorLeave
  SendMessage $OperatorList ${CB_GETCURSEL} 0 0 $Index
  ${If} $Index > 0
    IntOp $Index $Index - 1
    ReadINIStr $OperatorSid "$PLUGINSDIR\users.ini" Users "sid$Index"
  ${Else}
    StrCpy $OperatorSid ""
    IfFileExists "$PROGRAMFILES64\Quad9Connect\installation.json" upgrade_selected
    MessageBox MB_ICONEXCLAMATION "Choose the local user who will control DNS."
    Abort
    upgrade_selected:
  ${EndIf}
FunctionEnd

Section Install
  SetOutPath "$PLUGINSDIR"
  WriteUninstaller "$PLUGINSDIR\Uninstall.exe"
  nsExec::ExecToLog '"$SYSDIR\WindowsPowerShell\v1.0\powershell.exe" -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "$PLUGINSDIR\Manage-Quad9Installation.ps1" -Operation Install -Payload "$PLUGINSDIR" -OperatorSid "$OperatorSid"'
  Pop $0
  ${If} $0 != 0
    MessageBox MB_OK|MB_ICONSTOP "Installation stopped. Review the details above. If rollback is pending, run the retained administrator Repair command before retrying." /SD IDOK
    SetErrorLevel 1
    Abort
  ${EndIf}
SectionEnd

Function un.onInit
  ${DisableX64FSRedirection}
  SetRegView 64
FunctionEnd

Section Uninstall
  InitPluginsDir
  SetOutPath "$PLUGINSDIR"
  File "InstallerLifecycle.psm1"
  File "Manage-Quad9Installation.ps1"
  nsExec::ExecToLog '"$SYSDIR\WindowsPowerShell\v1.0\powershell.exe" -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "$PLUGINSDIR\Manage-Quad9Installation.ps1" -Operation Uninstall'
  Pop $0
  ${If} $0 != 0
    MessageBox MB_OK|MB_ICONSTOP "Removal stopped. The service and recovery records remain available. Resolve the reported conflict and retry, or run the administrator Repair command." /SD IDOK
    SetErrorLevel 1
    Abort
  ${EndIf}
SectionEnd
