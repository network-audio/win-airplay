Unicode true
SetCompressor /SOLID lzma

!include "MUI2.nsh"
!include "LogicLib.nsh"

!ifndef DIST_DIR
  !error "Pass -DDIST_DIR=<staging directory>"
!endif
!ifndef OUT_FILE
  !define OUT_FILE "win-airplay-usbip-windows-x64-setup.exe"
!endif

Name "Win AirPlay USB/IP Preview"
OutFile "${OUT_FILE}"
InstallDir "$PROGRAMFILES64\Win AirPlay USBIP"
RequestExecutionLevel admin

!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "English"

Section "Win AirPlay USB/IP" Main
  SetRegView 64
  SetOutPath "$INSTDIR"
  File "${DIST_DIR}\win-airplay.exe"
  File "${DIST_DIR}\install-usbip.ps1"
  File "${DIST_DIR}\run-usbip.ps1"
  File "${DIST_DIR}\rotating-log.ps1"
  File "${DIST_DIR}\test-playback.ps1"
  File "${DIST_DIR}\test-tone.wav"
  File "${DIST_DIR}\NYX-LICENSE.txt"
  File "${DIST_DIR}\USBIP-LICENSE.txt"
  File "${DIST_DIR}\USBip-0.9.8.1-x64.exe"

  nsExec::ExecToLog 'powershell.exe -NoProfile -ExecutionPolicy Bypass -File "$INSTDIR\install-usbip.ps1" -Action Install'
  Pop $0
  ${If} $0 != 0
    MessageBox MB_ICONSTOP "USB/IP installation or startup failed."
    Abort
  ${EndIf}

  WriteUninstaller "$INSTDIR\Uninstall.exe"
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\WinAirPlayUSBIP" "DisplayName" "Win AirPlay USB/IP Preview"
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\WinAirPlayUSBIP" "UninstallString" '"$INSTDIR\Uninstall.exe"'
SectionEnd

Section "Uninstall"
  nsExec::ExecToLog 'powershell.exe -NoProfile -ExecutionPolicy Bypass -File "$INSTDIR\install-usbip.ps1" -Action Uninstall'
  Pop $0
  Delete "$INSTDIR\win-airplay.exe"
  Delete "$INSTDIR\install-usbip.ps1"
  Delete "$INSTDIR\run-usbip.ps1"
  Delete "$INSTDIR\rotating-log.ps1"
  Delete "$INSTDIR\test-playback.ps1"
  Delete "$INSTDIR\test-tone.wav"
  Delete "$INSTDIR\NYX-LICENSE.txt"
  Delete "$INSTDIR\USBIP-LICENSE.txt"
  Delete "$INSTDIR\USBip-0.9.8.1-x64.exe"
  Delete "$INSTDIR\Uninstall.exe"
  RMDir "$INSTDIR"
  DeleteRegKey HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\WinAirPlayUSBIP"
SectionEnd
