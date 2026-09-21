; The Windows installer, built by Inno Setup 6.
;
; Built by `packaging/windows/package.ps1`, which passes the version in; this
; file is not a second place the version is written down. What it installs is the
; program and its documentation - no browser and no Xray, for the reasons in
; docs/release.md. It does not add itself to PATH either: this is a window with
; four command-line options for support, not a command-line tool, and a PATH
; entry is a change to the user's machine that nothing here needs.
;
; Privileges: `lowest` by default, so a user without administrator rights can
; install it. Inno's override dialog lets an administrator put it in Program
; Files instead, which is why the default directory is `{autopf}`.

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef SourceRoot
  #define SourceRoot "..\.."
#endif

#define AppName "Fingerprint Browser"
#define AppExeName "fingerprint-browser.exe"
#define AppPublisher "the Fingerprint Browser authors"

[Setup]
; The identity of this installation, not of the program: it has to stay the same
; across versions for an upgrade to replace the old one and for Add/Remove
; Programs to show one entry rather than two. Invented once, and never derived
; from the name, because the name can change.
AppId={{E5050C46-52C2-4FE7-AA69-9FEF59162BC8}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher={#AppPublisher}
DefaultDirName={autopf}\Fingerprint Browser
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
AllowNoIcons=yes
; The icon in the wizard, in Add/Remove Programs, and on the shortcuts.
SetupIconFile={#SourceRoot}\assets\icon.ico
UninstallDisplayIcon={app}\{#AppExeName}
OutputDir={#SourceRoot}\dist
OutputBaseFilename=fingerprint-browser-{#AppVersion}-windows-x86_64-setup
LicenseFile={#SourceRoot}\LICENSE
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
; `x64compatible` replaces the deprecated `x64` in Inno Setup 6.3+
; and also allows x64 emulation on Windows 11 ARM64.
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "Create a desktop shortcut"; GroupDescription: "Shortcuts:"

[Files]
Source: "{#SourceRoot}\target\release\{#AppExeName}"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceRoot}\README.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceRoot}\CHANGELOG.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceRoot}\LICENSE"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceRoot}\docs\*.md"; DestDir: "{app}\docs"; Flags: ignoreversion
Source: "{#SourceRoot}\assets\icon.png"; DestDir: "{app}\assets"; Flags: ignoreversion
Source: "{#SourceRoot}\assets\icon.svg"; DestDir: "{app}\assets"; Flags: ignoreversion

[Icons]
; The group entry is the way to the program; the desktop one is opt-in, because
; a shortcut nobody asked for is litter on somebody's desktop.
Name: "{group}\{#AppName}"; Filename: "{app}\{#AppExeName}"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExeName}"; Tasks: desktopicon

[Run]
Filename: "{app}\{#AppExeName}"; Description: "Launch {#AppName}"; Flags: nowait postinstall skipifsilent
