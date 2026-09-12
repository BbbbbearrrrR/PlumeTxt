; Build with scripts/build-setup.ps1 (Inno Setup 6.6 or later).
#ifndef AppVersion
  #error AppVersion must come from Cargo metadata
#endif

[Setup]
AppId={{89803657-528F-439F-944A-3C754712938E}
AppName=PlumeTxt
AppVersion={#AppVersion}
AppPublisher=PlumeTxt
AppPublisherURL=https://github.com/BbbbbearrrrR/PlumeTxt
DefaultDirName={localappdata}\Programs\PlumeTxt
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0.17763
OutputDir={#OutputDir}
OutputBaseFilename=PlumeTxt-{#AppVersion}-Setup-x64
SetupIconFile=..\assets\feather.ico
UninstallDisplayIcon={app}\PlumeTxt.exe
WizardStyle=modern dynamic
Compression=lzma2
SolidCompression=yes
ChangesAssociations=yes
CloseApplications=yes
RestartApplications=no
UninstallDisplayName=PlumeTxt
VersionInfoVersion={#AppVersion}

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"
Name: "chinesesimplified"; MessagesFile: "ChineseSimplified.isl"

[CustomMessages]
english.DesktopShortcut=Create a desktop shortcut
english.FileAssociations=Add PlumeTxt to Open with and Default apps (keep existing defaults)
english.FolderMenu=Add Open with PlumeTxt to folder context menus
chinesesimplified.DesktopShortcut=创建桌面快捷方式
chinesesimplified.FileAssociations=添加到“打开方式”和“默认应用”（保留现有默认设置）
chinesesimplified.FolderMenu=在文件夹右键菜单中添加“用 PlumeTxt 打开”

[Tasks]
Name: "desktopicon"; Description: "{cm:DesktopShortcut}"; Flags: unchecked
Name: "associations"; Description: "{cm:FileAssociations}"; Flags: unchecked
Name: "foldermenu"; Description: "{cm:FolderMenu}"; Flags: unchecked

[Files]
Source: "{#PayloadDir}\plumetxt.exe"; DestDir: "{app}"; DestName: "PlumeTxt.exe"; Flags: ignoreversion
Source: "..\runtime\pdfium\*"; DestDir: "{app}\runtime\pdfium"; Flags: ignoreversion recursesubdirs createallsubdirs
Source: "..\README.md"; DestDir: "{app}"; Flags: ignoreversion
#ifdef ShellMenuDir
Source: "{#ShellMenuDir}\PlumeTxt.ShellMenu.dll"; DestDir: "{app}"; Flags: ignoreversion; Tasks: foldermenu; MinVersion: 10.0.22000
Source: "{#ShellMenuDir}\PlumeTxt.ShellMenu.msix"; DestDir: "{app}"; Flags: ignoreversion; Tasks: foldermenu; MinVersion: 10.0.22000
Source: "{#ShellMenuDir}\ShellMenu.png"; DestDir: "{app}"; Flags: ignoreversion; Tasks: foldermenu; MinVersion: 10.0.22000
Source: "register-shell-menu.ps1"; DestDir: "{app}"; Flags: ignoreversion; Tasks: foldermenu; MinVersion: 10.0.22000
#endif

[Icons]
Name: "{autoprograms}\PlumeTxt"; Filename: "{app}\PlumeTxt.exe"; WorkingDir: "{userdocs}"
Name: "{autodesktop}\PlumeTxt"; Filename: "{app}\PlumeTxt.exe"; WorkingDir: "{userdocs}"; Tasks: desktopicon

[Run]
Filename: "{app}\PlumeTxt.exe"; WorkingDir: "{userdocs}"; Description: "{cm:LaunchProgram,PlumeTxt}"; Flags: nowait postinstall skipifsilent

; Native registry entries give Setup ownership and automatic uninstall cleanup.
#include GeneratedAssociations

[Code]
procedure ShellMenuRegistration(Remove: Boolean);
var
  Args: String;
  ResultCode: Integer;
begin
  if not FileExists(ExpandConstant('{app}\register-shell-menu.ps1')) then exit;
  Args := '-NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "' +
    ExpandConstant('{app}\register-shell-menu.ps1') + '"';
  if Remove then Args := Args + ' -Remove';
  if not Exec(ExpandConstant('{sys}\WindowsPowerShell\v1.0\powershell.exe'), Args,
    '', SW_HIDE, ewWaitUntilTerminated, ResultCode) or (ResultCode <> 0) then
  begin
    Log('PlumeTxt modern context menu registration failed. See %LOCALAPPDATA%\PlumeTxt\shell-menu-install.log');
    if not Remove and not WizardSilent then
      MsgBox('Windows could not register the PlumeTxt context menu. The classic menu remains available.' + #13#10 +
        'Details: %LOCALAPPDATA%\PlumeTxt\shell-menu-install.log', mbError, MB_OK);
  end;
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
#ifdef ShellMenuDir
  if CurStep = ssPostInstall then
    ShellMenuRegistration(not WizardIsTaskSelected('foldermenu'));
#endif
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usUninstall then ShellMenuRegistration(True);
end;
