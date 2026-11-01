; Inno Setup script for the Windows installer.
; Built in CI: iscc /DAppVersion=0.1.0 installer\windows\phpcraft.iss

#ifndef AppVersion
  #define AppVersion "0.1.0"
#endif
; Numeric file version (AppVersion may carry a suffix such as -alpha).
#ifndef NumVersion
  #define NumVersion "0.1.0"
#endif
; Folder under target\ holding phpcraft.exe
#ifndef BinDir
  #define BinDir "release"
#endif
; x64 or x86
#ifndef Arch
  #define Arch "x64"
#endif

[Setup]
AppId={{6F2C0E9B-4F1E-4C1B-9D2A-5A7E3C1B9F10}
AppName=PhpCraft
AppVersion={#AppVersion}
AppPublisher=PhpCraft (path.az)
AppPublisherURL=https://github.com/rasul-chopurov/phpcraft
DefaultDirName={autopf}\PhpCraft
DefaultGroupName=PhpCraft
UninstallDisplayIcon={app}\phpcraft.exe
SetupIconFile=..\..\assets\icon\phpcraft.ico
OutputDir=..\..\target\release
OutputBaseFilename=PhpCraft-{#AppVersion}-windows-{#Arch}-Setup
VersionInfoVersion={#NumVersion}
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
#if Arch == "x64"
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
#endif
PrivilegesRequiredOverridesAllowed=dialog
ChangesAssociations=yes

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"
Name: "russian"; MessagesFile: "compiler:Languages\Russian.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"
Name: "associate"; Description: "Open .php, .html, .sql and other source files with PhpCraft"; GroupDescription: "File associations:"

[Files]
Source: "..\..\target\{#BinDir}\phpcraft.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\README.md"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{group}\PhpCraft"; Filename: "{app}\phpcraft.exe"
Name: "{group}\{cm:UninstallProgram,PhpCraft}"; Filename: "{uninstallexe}"
Name: "{autodesktop}\PhpCraft"; Filename: "{app}\phpcraft.exe"; Tasks: desktopicon

[Registry]
; "Open with PhpCraft" for source files (a ProgID plus OpenWithProgids, so existing defaults are kept).
Root: HKA; Subkey: "Software\Classes\PhpCraft.Source"; ValueType: string; ValueName: ""; ValueData: "Source file"; Flags: uninsdeletekey; Tasks: associate
Root: HKA; Subkey: "Software\Classes\PhpCraft.Source\DefaultIcon"; ValueType: string; ValueName: ""; ValueData: "{app}\phpcraft.exe,0"; Tasks: associate
Root: HKA; Subkey: "Software\Classes\PhpCraft.Source\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\phpcraft.exe"" ""%1"""; Tasks: associate
Root: HKA; Subkey: "Software\Classes\.php\OpenWithProgids"; ValueType: string; ValueName: "PhpCraft.Source"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associate
Root: HKA; Subkey: "Software\Classes\.phtml\OpenWithProgids"; ValueType: string; ValueName: "PhpCraft.Source"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associate
Root: HKA; Subkey: "Software\Classes\.html\OpenWithProgids"; ValueType: string; ValueName: "PhpCraft.Source"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associate
Root: HKA; Subkey: "Software\Classes\.htm\OpenWithProgids"; ValueType: string; ValueName: "PhpCraft.Source"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associate
Root: HKA; Subkey: "Software\Classes\.css\OpenWithProgids"; ValueType: string; ValueName: "PhpCraft.Source"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associate
Root: HKA; Subkey: "Software\Classes\.js\OpenWithProgids"; ValueType: string; ValueName: "PhpCraft.Source"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associate
Root: HKA; Subkey: "Software\Classes\.ts\OpenWithProgids"; ValueType: string; ValueName: "PhpCraft.Source"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associate
Root: HKA; Subkey: "Software\Classes\.vue\OpenWithProgids"; ValueType: string; ValueName: "PhpCraft.Source"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associate
Root: HKA; Subkey: "Software\Classes\.json\OpenWithProgids"; ValueType: string; ValueName: "PhpCraft.Source"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associate
Root: HKA; Subkey: "Software\Classes\.xml\OpenWithProgids"; ValueType: string; ValueName: "PhpCraft.Source"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associate
Root: HKA; Subkey: "Software\Classes\.yml\OpenWithProgids"; ValueType: string; ValueName: "PhpCraft.Source"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associate
Root: HKA; Subkey: "Software\Classes\.yaml\OpenWithProgids"; ValueType: string; ValueName: "PhpCraft.Source"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associate
Root: HKA; Subkey: "Software\Classes\.md\OpenWithProgids"; ValueType: string; ValueName: "PhpCraft.Source"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associate
Root: HKA; Subkey: "Software\Classes\.sql\OpenWithProgids"; ValueType: string; ValueName: "PhpCraft.Source"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associate
Root: HKA; Subkey: "Software\Classes\.env\OpenWithProgids"; ValueType: string; ValueName: "PhpCraft.Source"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associate
Root: HKA; Subkey: "Software\Classes\.http\OpenWithProgids"; ValueType: string; ValueName: "PhpCraft.Source"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associate
; "Open Folder as PhpCraft Project" in Explorer.
Root: HKA; Subkey: "Software\Classes\Directory\shell\PhpCraft"; ValueType: string; ValueName: ""; ValueData: "Open as PhpCraft Project"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\Directory\shell\PhpCraft"; ValueType: string; ValueName: "Icon"; ValueData: "{app}\phpcraft.exe"
Root: HKA; Subkey: "Software\Classes\Directory\shell\PhpCraft\command"; ValueType: string; ValueName: ""; ValueData: """{app}\phpcraft.exe"" ""%1"""

[Run]
Filename: "{app}\phpcraft.exe"; Description: "{cm:LaunchProgram,PhpCraft}"; Flags: nowait postinstall skipifsilent
