Unicode true
Target amd64-unicode
SetCompressor /SOLID lzma

!include "MUI2.nsh"
!include "x64.nsh"
!include "LogicLib.nsh"

!ifndef VERSION
  !define VERSION "0.1.0"
!endif
!ifndef DIST_DIR
  !error "Pass -DDIST_DIR=<staging directory>"
!endif
!ifndef OUT_FILE
  !define OUT_FILE "win-airplay-${VERSION}-windows-x64-setup.exe"
!endif

Name "Win AirPlay"
OutFile "${OUT_FILE}"
InstallDir "$PROGRAMFILES64\Win AirPlay"
RequestExecutionLevel admin

!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "English"

Section "Win AirPlay" Main
  SetRegView 64
  SetOutPath "$INSTDIR"
  File "${DIST_DIR}\win-airplay.exe"
  File "${DIST_DIR}\nyxr.exe"
  File "${DIST_DIR}\install.ps1"
  File "${DIST_DIR}\NYX-LICENSE.txt"
  File "${DIST_DIR}\MICROSOFT-PUBLIC-LICENSE.txt"
  SetOutPath "$INSTDIR\driver"
  File "${DIST_DIR}\driver\ComponentizedAudioSample.inf"
  File "${DIST_DIR}\driver\TabletAudioSample.sys"
  File "${DIST_DIR}\driver\KeywordDetectorContosoAdapter.dll"
  File /nonfatal "${DIST_DIR}\driver\sysvad.cat"

  nsExec::ExecToLog 'powershell.exe -NoProfile -ExecutionPolicy Bypass -File "$INSTDIR\install.ps1" -Action Install'
  Pop $0
  ${If} $0 != 0
    MessageBox MB_ICONSTOP "Driver or startup installation failed. A Microsoft-signed driver package is required."
    Abort
  ${EndIf}

  WriteUninstaller "$INSTDIR\Uninstall.exe"
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\WinAirPlay" "DisplayName" "Win AirPlay"
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\WinAirPlay" "UninstallString" '"$INSTDIR\Uninstall.exe"'
SectionEnd

Section "Uninstall"
  nsExec::ExecToLog 'powershell.exe -NoProfile -ExecutionPolicy Bypass -File "$INSTDIR\install.ps1" -Action Uninstall'
  Pop $0
  Delete "$INSTDIR\win-airplay.exe"
  Delete "$INSTDIR\nyxr.exe"
  Delete "$INSTDIR\install.ps1"
  Delete "$INSTDIR\NYX-LICENSE.txt"
  Delete "$INSTDIR\MICROSOFT-PUBLIC-LICENSE.txt"
  Delete "$INSTDIR\driver\ComponentizedAudioSample.inf"
  Delete "$INSTDIR\driver\TabletAudioSample.sys"
  Delete "$INSTDIR\driver\KeywordDetectorContosoAdapter.dll"
  Delete "$INSTDIR\driver\sysvad.cat"
  RMDir "$INSTDIR\driver"
  Delete "$INSTDIR\Uninstall.exe"
  RMDir "$INSTDIR"
  DeleteRegKey HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\WinAirPlay"
SectionEnd
