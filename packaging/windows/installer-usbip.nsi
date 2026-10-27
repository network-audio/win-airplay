Unicode true
SetCompressor /SOLID lzma

!include "MUI2.nsh"
!include "LogicLib.nsh"

!ifndef DIST_DIR
  !error "Pass -DDIST_DIR=<staging directory>"
!endif
!ifndef OUT_FILE
  !define OUT_FILE "win-airplay-usbip-windows-setup.exe"
!endif
!ifndef USBIP_ARCH
  !error "Pass -DUSBIP_ARCH=x64 or arm64"
!endif

Name "Win AirPlay USB/IP"
OutFile "${OUT_FILE}"
InstallDir "$PROGRAMFILES64\Win AirPlay USBIP"
RequestExecutionLevel admin
VIProductVersion "1.0.1.0"
VIAddVersionKey "ProductVersion" "1.0.1"

!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "English"

Section "Win AirPlay USB/IP" Main
  UserInfo::GetAccountType
  Pop $0
  ${If} $0 != "Admin"
    MessageBox MB_ICONSTOP "Administrator permission is required. Please approve the Windows elevation prompt." /SD IDOK
    Abort
  ${EndIf}
  SetRegView 64
  SetOutPath "$INSTDIR"
  File "${DIST_DIR}\win-airplay.exe"
  File "${DIST_DIR}\install-usbip.ps1"
  File "${DIST_DIR}\find-usbip.ps1"
  File "${DIST_DIR}\run-usbip.ps1"
  File "${DIST_DIR}\rotating-log.ps1"
  File "${DIST_DIR}\test-playback.ps1"
  File "${DIST_DIR}\test-tone.wav"
  File "${DIST_DIR}\NYX-LICENSE.txt"
  File "${DIST_DIR}\USBIP-LICENSE.txt"
  File "${DIST_DIR}\USBip-0.9.8.1-${USBIP_ARCH}.exe"

  nsExec::ExecToLog '"$WINDIR\Sysnative\WindowsPowerShell\v1.0\powershell.exe" -NoProfile -ExecutionPolicy Bypass -File "$INSTDIR\install-usbip.ps1" -Action Install'
  Pop $0
  ${If} $0 != 0
    MessageBox MB_ICONSTOP "USB/IP installation or startup failed." /SD IDOK
    Abort
  ${EndIf}

  WriteUninstaller "$INSTDIR\Uninstall.exe"
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\WinAirPlayUSBIP" "DisplayName" "Win AirPlay USB/IP"
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\WinAirPlayUSBIP" "DisplayVersion" "1.0.1"
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\WinAirPlayUSBIP" "UninstallString" '"$INSTDIR\Uninstall.exe"'
SectionEnd

Section "Uninstall"
  nsExec::ExecToLog '"$WINDIR\Sysnative\WindowsPowerShell\v1.0\powershell.exe" -NoProfile -ExecutionPolicy Bypass -File "$INSTDIR\install-usbip.ps1" -Action Uninstall'
  Pop $0
  Delete "$INSTDIR\win-airplay.exe"
  Delete "$INSTDIR\install-usbip.ps1"
  Delete "$INSTDIR\find-usbip.ps1"
  Delete "$INSTDIR\usbip-path.txt"
  Delete "$INSTDIR\run-usbip.ps1"
  Delete "$INSTDIR\rotating-log.ps1"
  Delete "$INSTDIR\test-playback.ps1"
  Delete "$INSTDIR\test-tone.wav"
  Delete "$INSTDIR\NYX-LICENSE.txt"
  Delete "$INSTDIR\USBIP-LICENSE.txt"
  Delete "$INSTDIR\USBip-0.9.8.1-${USBIP_ARCH}.exe"
  Delete "$INSTDIR\Uninstall.exe"
  RMDir "$INSTDIR"
  DeleteRegKey HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\WinAirPlayUSBIP"
SectionEnd
