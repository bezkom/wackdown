; WackDown installer (Inno Setup 6 - https://jrsoftware.org/isinfo.php)
; Build with installer\build.ps1, or: ISCC.exe /DAppVersion=1.0.0 installer\wackdown.iss

#ifndef AppVersion
  #define AppVersion "1.0.0"
#endif
#define AppName "WackDown"
#define AppExe "wackdown.exe"
#define ProgId "WackDown.Document"

[Setup]
; AppId identifies the app for upgrades and uninstall - never change it.
AppId={{CD9FEF6C-1EE4-43C8-A706-0B81998F7A3A}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher=WackDown Core
; Per-user install by default (no UAC prompt); "/ALLUSERS" on the command line installs machine-wide.
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=commandline
DefaultDirName={autopf}\{#AppName}
DisableDirPage=yes
DisableProgramGroupPage=yes
DisableReadyPage=yes
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
ChangesAssociations=yes
UninstallDisplayIcon={app}\{#AppExe}
UninstallDisplayName={#AppName}
OutputDir=..\dist
OutputBaseFilename=wackdown-setup-{#AppVersion}
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
SetupIconFile=..\assets\icon\wackdown.ico
WizardSizePercent=100

[Tasks]
Name: "assoc"; Description: "Open .md and .markdown files with {#AppName}"
Name: "desktopicon"; Description: "Create a desktop shortcut"; Flags: unchecked

[Files]
Source: "..\target\x86_64-pc-windows-msvc\release\{#AppExe}"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\LICENSE"; DestDir: "{app}"; DestName: "LICENSE.txt"; Flags: ignoreversion
Source: "..\assets\fonts\Iosevka-LICENSE.md"; DestDir: "{app}"; DestName: "Iosevka-LICENSE.txt"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\{#AppName}"; Filename: "{app}\{#AppExe}"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExe}"; Tasks: desktopicon

[Registry]
; HKA = HKCU for per-user installs, HKLM for /ALLUSERS installs.
Root: HKA; Subkey: "Software\Classes\{#ProgId}"; ValueType: string; ValueName: ""; ValueData: "Markdown Document"; Flags: uninsdeletekey; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\{#ProgId}\DefaultIcon"; ValueType: string; ValueName: ""; ValueData: """{app}\{#AppExe}"",1"; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\{#ProgId}\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#AppExe}"" ""%1"""; Tasks: assoc
; Claim the extension only when nothing else handles it; otherwise just appear in "Open with"
; (Windows 10/11 then lets the user pick WackDown as the default once).
Root: HKA; Subkey: "Software\Classes\.md"; ValueType: string; ValueName: ""; ValueData: "{#ProgId}"; Tasks: assoc; Check: HasNoHandler('.md')
Root: HKA; Subkey: "Software\Classes\.md\OpenWithProgids"; ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\.markdown"; ValueType: string; ValueName: ""; ValueData: "{#ProgId}"; Tasks: assoc; Check: HasNoHandler('.markdown')
Root: HKA; Subkey: "Software\Classes\.markdown\OpenWithProgids"; ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue; Tasks: assoc
; Lets "Open with" list WackDown for markdown files even when it is not the default.
Root: HKA; Subkey: "Software\Classes\Applications\{#AppExe}"; ValueType: string; ValueName: "FriendlyAppName"; ValueData: "{#AppName}"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\Applications\{#AppExe}\SupportedTypes"; ValueType: string; ValueName: ".md"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\Applications\{#AppExe}\SupportedTypes"; ValueType: string; ValueName: ".markdown"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\Applications\{#AppExe}\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#AppExe}"" ""%1"""

[Run]
Filename: "{app}\{#AppExe}"; Description: "Launch {#AppName}"; Flags: nowait postinstall skipifsilent

[Code]
function HasNoHandler(Ext: String): Boolean;
var
  Current: String;
begin
  Result := not RegQueryStringValue(HKA, 'Software\Classes\' + Ext, '', Current) or (Current = '') or (Current = '{#ProgId}');
end;

procedure RemoveClaim(Ext: String);
var
  Current: String;
begin
  // Only undo the extension default if it still points at us.
  if RegQueryStringValue(HKA, 'Software\Classes\' + Ext, '', Current) and (Current = '{#ProgId}') then
    RegDeleteValue(HKA, 'Software\Classes\' + Ext, '');
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usUninstall then
  begin
    RemoveClaim('.md');
    RemoveClaim('.markdown');
  end;
end;
