; Elixir installer (Inno Setup 6). Built by the CI workflow:
;   iscc /DMyAppVersion=x.y.z installer\elixir.iss

#ifndef MyAppVersion
  #define MyAppVersion "0.0.0"
#endif
#define MyAppName "Elixir"
#define MyAppExe "elixir.exe"
#define MyAppPublisher "Elixir"

[Setup]
AppId={{14851E78-E19D-46A9-A2A7-7042147BD48A}
AppName={#MyAppName}
AppVersion={#MyAppVersion}
AppVerName={#MyAppName} {#MyAppVersion}
AppPublisher={#MyAppPublisher}
; Per-user install: no admin prompt, lands in %LOCALAPPDATA%\Programs\Elixir.
PrivilegesRequired=lowest
DefaultDirName={autopf}\{#MyAppName}
DefaultGroupName={#MyAppName}
DisableProgramGroupPage=yes
UninstallDisplayIcon={app}\{#MyAppExe}
UninstallDisplayName={#MyAppName}
SetupIconFile=..\windows\elixir.ico
WizardStyle=modern
WizardSmallImageFile=wizard-small.bmp
WizardImageFile=wizard-large.bmp
OutputDir=..\target\installer
OutputBaseFilename=Elixir-Setup-{#MyAppVersion}
Compression=lzma2/max
SolidCompression=yes
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
; Setup closes a running Elixir itself (see CloseElixir below) instead of
; stopping to ask, so an install never waits on a copy hidden in the tray.
CloseApplications=force
RestartApplications=no
LicenseFile=..\LICENSE

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "Create a &desktop shortcut"; GroupDescription: "Shortcuts:"
Name: "autostart"; Description: "Start Elixir with &Windows (in the tray, so automatic renaming keeps working)"; GroupDescription: "Integration:"; Flags: unchecked

[Files]
Source: "..\target\release\{#MyAppExe}"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\README.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\LICENSE"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\{#MyAppName}"; Filename: "{app}\{#MyAppExe}"
Name: "{autodesktop}\{#MyAppName}"; Filename: "{app}\{#MyAppExe}"; Tasks: desktopicon

[Registry]
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "Elixir"; ValueData: """{app}\{#MyAppExe}"" --tray"; Tasks: autostart
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: none; ValueName: "Elixir"; Flags: uninsdeletevalue

[Run]
Filename: "{app}\{#MyAppExe}"; Description: "Launch Elixir"; Flags: nowait postinstall skipifsilent
; After an update from inside Elixir (silent install), start it again.
Filename: "{app}\{#MyAppExe}"; Flags: nowait; Check: WizardSilent

[Code]
const
  ElixirMutex = 'ElixirSingleInstance';
  WebView2Key = 'SOFTWARE\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}';

function ElixirGone(Steps: Integer): Boolean;
var
  I: Integer;
begin
  for I := 1 to Steps do
  begin
    if not CheckForMutexes(ElixirMutex) then
    begin
      Result := True;
      Exit;
    end;
    Sleep(250);
  end;
  Result := not CheckForMutexes(ElixirMutex);
end;

{ Ask a running Elixir to finish any rename and quit; force it after 15 seconds. }
function CloseElixir(): Boolean;
var
  DataDir: String;
  ResultCode: Integer;
begin
  Result := True;
  if not CheckForMutexes(ElixirMutex) then
    Exit;
  DataDir := ExpandConstant('{userappdata}\Elixir');
  ForceDirectories(DataDir);
  SaveStringToFile(DataDir + '\quit.request', '1', False);
  if not ElixirGone(60) then
  begin
    Exec(ExpandConstant('{sys}\taskkill.exe'), '/F /IM {#MyAppExe}', '', SW_HIDE, ewWaitUntilTerminated, ResultCode);
    Result := ElixirGone(40);
  end;
  DeleteFile(DataDir + '\quit.request');
end;

function HasWebView2(): Boolean;
var
  V: String;
begin
  Result :=
    (RegQueryStringValue(HKLM, 'SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}', 'pv', V) and (V <> '') and (V <> '0.0.0.0')) or
    (RegQueryStringValue(HKLM, WebView2Key, 'pv', V) and (V <> '') and (V <> '0.0.0.0')) or
    (RegQueryStringValue(HKCU, WebView2Key, 'pv', V) and (V <> '') and (V <> '0.0.0.0'));
end;

function InitializeSetup(): Boolean;
begin
  Result := True;
  if not HasWebView2() and not WizardSilent then
    MsgBox('Elixir shows its window with Microsoft Edge WebView2, which Windows 11 includes.' + #13#10 + #13#10 +
           'It wasn''t found on this PC. Setup will continue; if Elixir''s window stays blank, install the "Evergreen WebView2 Runtime" from Microsoft''s WebView2 page.',
           mbInformation, MB_OK);
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
begin
  Result := '';
  if not CloseElixir() then
    Result := 'Elixir is still running and could not be closed. Quit it from its tray icon, then click Back and Next.';
end;

function InitializeUninstall(): Boolean;
begin
  Result := CloseElixir();
  if not Result and not UninstallSilent then
    MsgBox('Elixir is still running and could not be closed. Quit it from its tray icon and run the uninstaller again.', mbError, MB_OK);
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  DataDir: String;
begin
  if CurUninstallStep = usPostUninstall then
  begin
    DataDir := ExpandConstant('{userappdata}\Elixir');
    if DirExists(DataDir) and not UninstallSilent then
      if MsgBox('Also delete Elixir''s settings, history and undo records?' + #13#10 + #13#10 +
                'Your video files are not touched either way.',
                mbConfirmation, MB_YESNO or MB_DEFBUTTON2) = IDYES then
        DelTree(DataDir, True, True, True);
  end;
end;
