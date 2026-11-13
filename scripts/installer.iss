; Inno Setup script for AI Video Studio: one Setup.exe that installs the app (FFmpeg included),
; adds a Start menu shortcut and an uninstaller, and opens .aivp projects with the app.
; Built by scripts\package-studio.ps1 -Installer, which prepares dist\AI Video Studio\ and the
; icon in dist\installer\ (both found relative to this file) and passes the version:
;   ISCC /DAppVersion=0.2.0 scripts\installer.iss

#define AppName "AI Video Studio"
#define AppExe "ai-video-studio.exe"
#define AppPublisher "AI Video Studio"
#define AppUrl "https://github.com/"
#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef SourceDir
  #define SourceDir "..\dist\AI Video Studio"
#endif
#ifndef IconFile
  #define IconFile "..\dist\installer\ai-video-studio.ico"
#endif
#ifndef OutputDir
  #define OutputDir "..\dist"
#endif

[Setup]
; Identifies the app across versions: a newer Setup.exe upgrades the installed one in place.
AppId={{8F3B6C2E-5D1A-4E7B-9C42-A1D0E6B7F359}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher={#AppPublisher}
VersionInfoVersion={#AppVersion}
VersionInfoDescription={#AppName} Setup
VersionInfoProductName={#AppName}
; Installs for the current user (no administrator prompt). "Install for all users" stays
; available from the first page for shared computers.
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=commandline
DefaultDirName={autopf}\{#AppName}
DisableProgramGroupPage=yes
DisableDirPage=auto
DisableReadyPage=yes
DisableWelcomePage=yes
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
OutputDir={#OutputDir}
OutputBaseFilename=AI-Video-Studio-Windows-Setup
SetupIconFile={#IconFile}
UninstallDisplayIcon={app}\{#AppExe}
UninstallDisplayName={#AppName}
WizardStyle=modern
Compression=lzma2/max
SolidCompression=yes
; Updating while the app is open: Setup asks to close it (projects save themselves).
CloseApplications=yes
RestartApplications=no
ChangesAssociations=yes

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "Create a &desktop shortcut"; GroupDescription: "Shortcuts:"

[Files]
Source: "{#SourceDir}\{#AppExe}"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\ffmpeg\*"; DestDir: "{app}\ffmpeg"; Flags: ignoreversion recursesubdirs createallsubdirs
Source: "{#SourceDir}\licenses\*"; DestDir: "{app}\licenses"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{autoprograms}\{#AppName}"; Filename: "{app}\{#AppExe}"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExe}"; Tasks: desktopicon

[Registry]
; Double-clicking a .aivp project opens it in the app.
Root: HKA; Subkey: "Software\Classes\.aivp\OpenWithProgids"; ValueType: string; ValueName: "AIVideoStudio.Project"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\AIVideoStudio.Project"; ValueType: string; ValueName: ""; ValueData: "{#AppName} project"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\AIVideoStudio.Project\DefaultIcon"; ValueType: string; ValueName: ""; ValueData: "{app}\{#AppExe},0"
Root: HKA; Subkey: "Software\Classes\AIVideoStudio.Project\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#AppExe}"" ""%1"""
Root: HKA; Subkey: "Software\Classes\Applications\{#AppExe}\SupportedTypes"; ValueType: string; ValueName: ".aivp"; ValueData: ""; Flags: uninsdeletekey

[Run]
Filename: "{app}\{#AppExe}"; Description: "Open {#AppName}"; Flags: nowait postinstall skipifsilent
