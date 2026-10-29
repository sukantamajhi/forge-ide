; Forge installer (Inno Setup 6). Build with installer\build.ps1.
; Installs for the current user only (no admin prompt) to %LOCALAPPDATA%\Programs\Forge.

#define AppName "Forge"
#define AppVersion "0.1.0"
#define AppExe "forge.exe"

[Setup]
AppId={{6E1B7C52-3F0A-4D1E-9C55-4F0B7A2E8D31}
AppName={#AppName}
AppVersion={#AppVersion}
AppPublisher=Forge
DefaultDirName={autopf}\{#AppName}
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
OutputDir=..\dist
OutputBaseFilename=Forge-Setup-{#AppVersion}
SetupIconFile=..\assets\forge.ico
UninstallDisplayIcon={app}\{#AppExe}
UninstallDisplayName={#AppName}
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
ChangesEnvironment=yes
ChangesAssociations=yes
CloseApplications=yes
RestartApplications=no

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked
Name: "contextfiles"; Description: "Add ""Open with Forge"" action to the file context menu"; GroupDescription: "Other:"
Name: "contextfolders"; Description: "Add ""Open with Forge"" action to the folder context menu"; GroupDescription: "Other:"
Name: "addtopath"; Description: "Add to PATH (so you can run ""forge ."" from a terminal)"; GroupDescription: "Other:"

[Files]
Source: "..\target\release\forge.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\assets\forge.ico"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{group}\{#AppName}"; Filename: "{app}\{#AppExe}"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExe}"; Tasks: desktopicon

[Registry]
; Right-click a file: Open with Forge
Root: HKA; Subkey: "Software\Classes\*\shell\OpenWithForge"; ValueType: string; ValueName: ""; ValueData: "Open with Forge"; Flags: uninsdeletekey; Tasks: contextfiles
Root: HKA; Subkey: "Software\Classes\*\shell\OpenWithForge"; ValueType: string; ValueName: "Icon"; ValueData: """{app}\{#AppExe}"""; Tasks: contextfiles
Root: HKA; Subkey: "Software\Classes\*\shell\OpenWithForge\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#AppExe}"" ""%1"""; Tasks: contextfiles
; Right-click a folder: Open with Forge
Root: HKA; Subkey: "Software\Classes\Directory\shell\OpenWithForge"; ValueType: string; ValueName: ""; ValueData: "Open with Forge"; Flags: uninsdeletekey; Tasks: contextfolders
Root: HKA; Subkey: "Software\Classes\Directory\shell\OpenWithForge"; ValueType: string; ValueName: "Icon"; ValueData: """{app}\{#AppExe}"""; Tasks: contextfolders
Root: HKA; Subkey: "Software\Classes\Directory\shell\OpenWithForge\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#AppExe}"" ""%1"""; Tasks: contextfolders
; Right-click the empty space inside a folder window
Root: HKA; Subkey: "Software\Classes\Directory\Background\shell\OpenWithForge"; ValueType: string; ValueName: ""; ValueData: "Open with Forge"; Flags: uninsdeletekey; Tasks: contextfolders
Root: HKA; Subkey: "Software\Classes\Directory\Background\shell\OpenWithForge"; ValueType: string; ValueName: "Icon"; ValueData: """{app}\{#AppExe}"""; Tasks: contextfolders
Root: HKA; Subkey: "Software\Classes\Directory\Background\shell\OpenWithForge\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#AppExe}"" ""%V"""; Tasks: contextfolders
; Listed in the "Open with" dialog for any file
Root: HKA; Subkey: "Software\Classes\Applications\{#AppExe}"; ValueType: string; ValueName: "FriendlyAppName"; ValueData: "{#AppName}"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\Applications\{#AppExe}\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#AppExe}"" ""%1"""
; PATH
Root: HKCU; Subkey: "Environment"; ValueType: expandsz; ValueName: "Path"; ValueData: "{olddata};{app}"; Check: NeedsPath(ExpandConstant('{app}')); Tasks: addtopath

[Run]
Filename: "{app}\{#AppExe}"; Description: "{cm:LaunchProgram,{#StringChange(AppName, '&', '&&')}}"; Flags: nowait postinstall skipifsilent

[Code]
function NeedsPath(Dir: string): Boolean;
var
  Current: string;
begin
  if not RegQueryStringValue(HKCU, 'Environment', 'Path', Current) then
  begin
    Result := True;
    Exit;
  end;
  Result := Pos(';' + Uppercase(Dir) + ';', ';' + Uppercase(Current) + ';') = 0;
end;

// Take the install folder back out of PATH when uninstalling.
procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  Current, Dir, Wrapped: string;
  P: Integer;
begin
  if CurUninstallStep <> usPostUninstall then Exit;
  if not RegQueryStringValue(HKCU, 'Environment', 'Path', Current) then Exit;
  Dir := ExpandConstant('{app}');
  Wrapped := ';' + Current + ';';
  P := Pos(';' + Uppercase(Dir) + ';', Uppercase(Wrapped));
  if P = 0 then Exit;
  Delete(Wrapped, P, Length(Dir) + 1);
  // Wrapped is ";a;b;" again: strip the added outer separators.
  Current := Copy(Wrapped, 2, Length(Wrapped) - 2);
  RegWriteExpandStringValue(HKCU, 'Environment', 'Path', Current);
end;
