; Agent Relay's Windows Setup program (Inno Setup 6.3 or later).
;
; scripts/build-windows.sh and scripts/package-windows.py compile it after
; the zips:
;
;   ISCC.exe /DVersion=<version> /DArch=x64|arm64 /DSourceDir=<tree> /DOutputDir=<dir>
;            [/DProductName=<name> /DUrlScheme=<scheme> /DAssetPrefix=<prefix>
;             /DSiteUrl=<origin> /DSupportUrl=<url> /DReleaseFeed=<feed>] AgentRelay.iss
;
; SourceDir is the built AgentRelay-Windows-<arch> tree. The result is
; <OutputDir>\<AssetPrefix>-<version>-Windows-<arch>-Setup.exe.
;
; A signed build (scripts/package-windows.py --sign-command) also passes
; /S<name>=<command> and /DSignTool=<name>: Inno then signs Setup and the
; uninstaller it embeds (unins000.exe) with that sign tool.
;
; The optional defines are the build's site profile (docs/workspace-layout.md,
; "Site profiles"): the product name the installer shows, the scheme of the
; links it registers a handler for, what the release's artifact names start
; with, the publisher and support links and the release feed. Each defaults
; to the default profile's (Agent Relay, agentrelay, AgentRelay,
; github:AgentWorkforce/relay-desktop-releases), so a build without them
; is the Setup program every production install was made with. Every profile
; keeps the same AppId, keys and install root: there is one Agent Relay app
; per machine, and a profile's Setup replaces whichever product is installed
; as an upgrade would (see AppGuid below). The support link defaults to the
; releases page of the default profile's repository, or, for a cloud release
; feed (ReleaseFeed "cloud"), to the profile site's downloads page,
; <SiteUrl>/cloud/dashboard/downloads.
;
; Setup installs per user, without administrator rights, exactly where the
; zip goes and the in-app updater expects it (install/windows.rs,
; updates/windows.rs):
;
;   %LOCALAPPDATA%\Programs\AgentRelay\<version>\AgentRelay-Windows-<arch>\
;
; so a later in-app update unpacks the next version beside it. The
; uninstaller lives one level up, in %LOCALAPPDATA%\Programs\AgentRelay, so it
; outlives the version folder it was installed with, and it removes every
; version, the Start menu shortcut, the build's link handler, the
; notification identity, the app's own launch-at-login value and the
; "Agent Relay (<account>)" logon task. It never touches your data:
; %USERPROFILE%\.agentworkforce and %APPDATA%\agent-relay stay.

#ifndef Version
  #error Pass /DVersion=<version>
#endif
#ifndef Arch
  #error Pass /DArch=x64 or /DArch=arm64
#endif
#ifndef SourceDir
  #error Pass /DSourceDir=<the AgentRelay-Windows-<arch> tree>
#endif
#ifndef OutputDir
  #define OutputDir "."
#endif
#ifndef ProductName
  #define ProductName "Agent Relay"
#endif
#ifndef UrlScheme
  #define UrlScheme "agentrelay"
#endif
#ifndef AssetPrefix
  #define AssetPrefix "AgentRelay"
#endif
#ifndef SiteUrl
  #define SiteUrl "https://agentrelay.com"
#endif
#ifndef ReleaseFeed
  #define ReleaseFeed "github:AgentWorkforce/relay-desktop-releases"
#endif
#ifndef SupportUrl
  #if ReleaseFeed == "cloud"
    #define SupportUrl SiteUrl + "/cloud/dashboard/downloads"
  #else
    #define SupportUrl "https://github.com/AgentWorkforce/relay-desktop-releases"
  #endif
#endif

#if Arch == "x64"
  #define Architectures "x64compatible"
#elif Arch == "arm64"
  #define Architectures "arm64"
#else
  #error Arch must be x64 or arm64
#endif

; Windows' file version is four numbers: a prerelease suffix is dropped.
#if Pos("-", Version) > 0
  #define NumericVersion Copy(Version, 1, Pos("-", Version) - 1)
#else
  #define NumericVersion Version
#endif

; Fixed for every release and every profile: one Agent Relay app per
; machine, so Windows lists one install of it, and Setup upgrades it. The
; uninstall entry is
; HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\{<AppGuid>}_is1
; (install::windows::UNINSTALL_KEY), which an in-app update keeps on the
; version it installs; the core names that key, the install root, the
; Start menu shortcut, the notification identity and the logon task for
; every build, so they are the same here whatever the profile. Setup
; installs over another product's install (a profile's Setup over Agent
; Relay, Agent Relay's over a profile's, or one profile's over another's)
; exactly as over an older version of its own, silently, and replaces it:
; the uninstall entry takes this build's name (UninstallDisplayName), the
; notification identity's name and the Start menu shortcut follow when
; --install-launcher runs ([Run]), the logon task and the Run value move to
; this version (CurStepChanged), and what the replaced product left that
; names it is removed (RemoveReplacedProduct). See docs/workspace-layout.md,
; "Site profiles".
#define AppGuid "C5377993-AE99-466F-A519-676A299A6C6D"
#define Tree "AgentRelay-Windows-" + Arch

[Setup]
#ifdef SignTool
SignTool={#SignTool}
SignedUninstaller=yes
#endif
AppId={{{#AppGuid}}
AppName={#ProductName}
AppVersion={#Version}
AppVerName={#ProductName} {#Version}
AppPublisher=Agent Relay
AppPublisherURL={#SiteUrl}
AppSupportURL={#SupportUrl}
VersionInfoVersion={#NumericVersion}
VersionInfoProductName={#ProductName}
VersionInfoDescription={#ProductName} Setup
; Per user, no UAC prompt.
PrivilegesRequired=lowest
ArchitecturesAllowed={#Architectures}
ArchitecturesInstallIn64BitMode={#Architectures}
; Each version in its own folder, as the in-app updater lays them out; never
; the folder an earlier version was installed in.
DefaultDirName={localappdata}\Programs\AgentRelay\{#Version}\{#Tree}
UsePreviousAppDir=no
DisableDirPage=yes
DisableProgramGroupPage=yes
DisableReadyPage=yes
UninstallFilesDir={localappdata}\Programs\AgentRelay
UninstallDisplayName={#ProductName}
UninstallDisplayIcon={app}\bin\agent-relay.exe
#if FileExists(AddBackslash(SourceDir) + "share\icons\agent-relay.ico")
SetupIconFile={#SourceDir}\share\icons\agent-relay.ico
#endif
WizardStyle=modern
OutputDir={#OutputDir}
OutputBaseFilename={#AssetPrefix}-{#Version}-Windows-{#Arch}-Setup
Compression=lzma2/max
SolidCompression=yes
CloseApplications=yes
RestartApplications=no

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#SourceDir}\*"; DestDir: "{app}"; Flags: recursesubdirs createallsubdirs ignoreversion

[Icons]
; The Start menu path the app writes itself (install::windows::SHORTCUT, the
; core's name for every build), so the two agree. The desktop shortcut is
; Setup's own, so it carries the product name.
Name: "{userprograms}\Agent Relay"; Filename: "{app}\bin\agent-relay.exe"; WorkingDir: "{app}\bin"; Comment: "{#ProductName}"
Name: "{userdesktop}\{#ProductName}"; Filename: "{app}\bin\agent-relay.exe"; WorkingDir: "{app}\bin"; Comment: "{#ProductName}"; Tasks: desktopicon

[Run]
; The shortcut, the {#UrlScheme}:// handler and the notification identity,
; as the app writes them on its first start, also after a silent install.
Filename: "{app}\bin\agent-relay.exe"; Parameters: "--install-launcher"; Flags: runhidden waituntilterminated
Filename: "{app}\bin\agent-relay.exe"; Description: "{cm:LaunchProgram,{#ProductName}}"; Flags: nowait postinstall skipifsilent

[UninstallDelete]
; Every version, including those in-app updates installed, and the
; uninstaller's own folder. Never %APPDATA%\agent-relay or ~\.agentworkforce.
Type: filesandordirs; Name: "{localappdata}\Programs\AgentRelay"
Type: files; Name: "{userprograms}\Agent Relay.lnk"
Type: files; Name: "{userdesktop}\{#ProductName}.lnk"

[Code]
const
  { The handler of the build's own link scheme, which the app registers too
    (--install-launcher; install::windows::protocol_key). }
  ProtocolKey = 'Software\Classes\{#UrlScheme}';
  AppUserModelKey = 'Software\Classes\AppUserModelId\AgentWorkforce.AgentRelay';
  RunKey = 'Software\Microsoft\Windows\CurrentVersion\Run';
  RunValue = 'AgentRelay';
  { The uninstall entry every build shares (AppId above). }
  UninstallKey = 'Software\Microsoft\Windows\CurrentVersion\Uninstall\{{#AppGuid}}_is1';
  ClassesKey = 'Software\Classes';

var
  { The product this Setup replaces (InitializeSetup), or ''. }
  ReplacedProduct: String;

{ %LOCALAPPDATA%\Programs\AgentRelay, lowercase, with a trailing backslash. }
function InstallRoot: String;
begin
  Result := Lowercase(AddBackslash(ExpandConstant('{localappdata}\Programs\AgentRelay')));
end;

{ The logon task's name, as service::windows::name builds it from USERNAME:
  "Agent Relay (alice)", or "Agent Relay" without a user name. }
function TaskName: String;
var
  User: String;
begin
  User := GetEnv('USERNAME');
  if User = '' then
    Result := 'Agent Relay'
  else
    Result := 'Agent Relay (' + User + ')';
end;

{ value as a PowerShell single-quoted string. }
function PsQuote(Value: String): String;
begin
  StringChangeEx(Value, '''', '''''', True);
  Result := '''' + Value + '''';
end;

{ Runs a PowerShell command hidden and waits for it; the exit code, or -1. }
function PowerShell(Command: String): Integer;
var
  Code: Integer;
begin
  if Exec(ExpandConstant('{sys}\WindowsPowerShell\v1.0\powershell.exe'),
    '-NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -Command "' + Command + '"',
    '', SW_HIDE, ewWaitUntilTerminated, Code) then
    Result := Code
  else
    Result := -1;
end;

{ Ends the Agent Relay programs running from the install root: the window
  only (agent-relay.exe without --headless), or every program there. }
procedure StopRunning(Everything: Boolean);
var
  Filter: String;
begin
  if Everything then
    Filter := '$true'
  else
    Filter := '($_.Name -ieq ''agent-relay.exe'' -and $_.CommandLine -notmatch ''--headless'')';
  PowerShell('$root = ' + PsQuote(InstallRoot) + '; ' +
    'Get-CimInstance Win32_Process | Where-Object { $_.ExecutablePath -and ' +
    '$_.ExecutablePath.ToLower().StartsWith($root) -and ' + Filter + ' } | ' +
    'ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }');
end;

{ The product a Setup install already here was made by, when it is not
  this build's: its uninstall entry's name, read before this install
  rewrites it. A zip or in-app-updater install has no entry and made no
  desktop shortcut, so it leaves this empty. }
function ReplacedProductName: String;
begin
  if not RegQueryStringValue(HKCU, UninstallKey, 'DisplayName', Result) or
    (Result = '{#ProductName}') then
    Result := '';
end;

{ Installing over another product needs no confirmation: Setup goes on as
  an upgrade. Only the replaced product's name is kept, for
  RemoveReplacedProduct. }
function InitializeSetup: Boolean;
begin
  ReplacedProduct := ReplacedProductName;
  Result := True;
end;

{ An open window of an older version would take the new one's launch (one
  instance per account), so it is closed first. The background service
  keeps running; it moves to the new version after the files are in place. }
function PrepareToInstall(var NeedsRestart: Boolean): String;
begin
  StopRunning(False);
  Result := '';
end;

{ As an in-app update does (updates::windows::repoint_task): the logon task,
  if there is one, runs the new version, keeping whether it is on, and a
  running task restarts on it. The action is replaced whole, program and
  arguments: a task windows/install.ps1 registered runs the bare core with
  `serve --headless`, which agent-relay.exe does not take, so its
  arguments become `--headless` here. A task the person never created is
  left to the app, which registers it off on its first start. False when
  the task could not be moved or restarted. }
function RepointTask: Boolean;
var
  Exe: String;
  Bin: String;
begin
  Exe := ExpandConstant('{app}\bin\agent-relay.exe');
  Bin := ExpandConstant('{app}\bin');
  Result := PowerShell('$ErrorActionPreference = ''Stop''; ' +
    '$task = Get-ScheduledTask -TaskPath ''\'' -TaskName ' + PsQuote(TaskName) + ' -ErrorAction SilentlyContinue; ' +
    'if (-not $task -or ($task.Actions[0].Execute -ieq ' + PsQuote(Exe) + ' -and $task.Actions[0].Arguments -eq ''--headless'')) { exit 0 }; ' +
    '$running = $task.State -eq ''Running''; ' +
    'if ($running) { Stop-ScheduledTask -InputObject $task }; ' +
    '$task.Actions[0].Execute = ' + PsQuote(Exe) + '; ' +
    '$task.Actions[0].Arguments = ''--headless''; ' +
    '$task.Actions[0].WorkingDirectory = ' + PsQuote(Bin) + '; ' +
    'Set-ScheduledTask -InputObject $task | Out-Null; ' +
    'if ($running) { Start-ScheduledTask -TaskPath ''\'' -TaskName ' + PsQuote(TaskName) + ' }') = 0;
end;

{ Whether a registry command runs a program under the install root. }
function RunsFromRoot(Command: String): Boolean;
begin
  Result := Pos(InstallRoot, Lowercase(Command)) > 0;
end;

{ The Run value this app writes: "<...>\agent-relay.exe" --background
  (relay::login_item::windows::owned). }
function RunValueIsOurs(Command: String): Boolean;
begin
  Command := Lowercase(Command);
  Result := (Pos('agent-relay.exe', Command) > 0) and (Pos('--background', Command) > 0);
end;

{ As an in-app update does (relay::login_item::windows::follow): Launch at
  login, when it is this app's own Run value, opens the new version at the
  next sign-in. Someone else's value, and no value, are left alone. }
procedure RepointRunValue;
var
  Command: String;
  Exe: String;
begin
  Exe := ExpandConstant('{app}\bin\agent-relay.exe');
  if RegQueryStringValue(HKCU, RunKey, RunValue, Command) and RunValueIsOurs(Command) then
    RegWriteStringValue(HKCU, RunKey, RunValue, '"' + Exe + '" --background');
end;

{ What another product this Setup replaced left that names it. Its link
  handler: a profile registers its own scheme, so the replaced product's
  key would go on opening that product's version folder, which stays under
  the install root; every URL protocol key under HKCU\Software\Classes
  whose command runs a program under the install root and that is not this
  build's scheme (--install-launcher's) is removed. Someone else's handlers
  run from elsewhere and are kept. Its desktop shortcut, which Setup named
  after it, when a Setup install was replaced. }
procedure RemoveReplacedProduct;
var
  Names: TArrayOfString;
  I: Integer;
  Key: String;
  Command: String;
begin
  if RegGetSubkeyNames(HKCU, ClassesKey, Names) then
    for I := 0 to GetArrayLength(Names) - 1 do
    begin
      Key := ClassesKey + '\' + Names[I];
      if (CompareText(Names[I], '{#UrlScheme}') <> 0) and
        RegValueExists(HKCU, Key, 'URL Protocol') and
        RegQueryStringValue(HKCU, Key + '\shell\open\command', '', Command) and
        RunsFromRoot(Command) then
        RegDeleteKeyIncludingSubkeys(HKCU, Key);
    end;
  if ReplacedProduct <> '' then
    DeleteFile(ExpandConstant('{userdesktop}\') + ReplacedProduct + '.lnk');
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  if CurStep = ssPostInstall then
  begin
    RepointRunValue;
    RemoveReplacedProduct;
    if not RepointTask then
      SuppressibleMsgBox(
        'The background service could not be moved to this version of {#ProductName}.' + #13#10 + #13#10 +
        'Open {#ProductName}, go to Settings and turn Background service off and on again to restart it.',
        mbError, MB_OK, IDOK);
  end;
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  Code: Integer;
  Command: String;
begin
  if CurUninstallStep = usUninstall then
  begin
    { Uninstalling is the one time the logon task is ended and removed. }
    Exec(ExpandConstant('{sys}\schtasks.exe'), '/End /TN "' + TaskName + '"', '',
      SW_HIDE, ewWaitUntilTerminated, Code);
    Exec(ExpandConstant('{sys}\schtasks.exe'), '/Delete /TN "' + TaskName + '" /F', '',
      SW_HIDE, ewWaitUntilTerminated, Code);
    StopRunning(True);
  end;
  if CurUninstallStep = usPostUninstall then
  begin
    { Only entries that run this install; someone else's are kept. }
    if RegQueryStringValue(HKCU, ProtocolKey + '\shell\open\command', '', Command) and
      RunsFromRoot(Command) then
      RegDeleteKeyIncludingSubkeys(HKCU, ProtocolKey);
    RegDeleteKeyIncludingSubkeys(HKCU, AppUserModelKey);
    if RegQueryStringValue(HKCU, RunKey, RunValue, Command) and RunValueIsOurs(Command) then
      RegDeleteValue(HKCU, RunKey, RunValue);
  end;
end;
