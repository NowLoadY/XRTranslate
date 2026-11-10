; Inno Setup owns registration, upgrades, shortcuts and uninstall.
#ifndef PackageDir
  #error PackageDir is required
#endif
#define AppId "{3C663FA0-7FD7-46C4-9C95-2BCA36A865B2}"

[Setup]
AppId={{#AppId}
AppName=XRTranslate
AppVersion={#AppVersion}
AppPublisher=NowLoadY
AppPublisherURL=https://github.com/NowLoadY/XRTranslate
AppSupportURL=https://github.com/NowLoadY/XRTranslate/issues
AppUpdatesURL=https://github.com/NowLoadY/XRTranslate/releases
DefaultDirName={localappdata}\Programs\XRTranslate
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0.19041
UsePreviousAppDir=yes
DisableWelcomePage=yes
DisableDirPage=no
DisableProgramGroupPage=yes
DisableReadyPage=yes
WizardStyle=modern light
WizardSizePercent=105,100
SetupIconFile={#PackageDir}\resources\branding\xrtranslate-logo.ico
WizardSmallImageFile={#PackageDir}\resources\branding\xrtranslate-logo.png
UninstallDisplayIcon={app}\XRTranslate.exe
UninstallDisplayName=XRTranslate
CloseApplications=yes
; The host owns plugin processes and shuts them down with the application.
CloseApplicationsFilter=XRTranslate.exe,XRTranslate-v*.exe
RestartApplications=no
OutputDir={#OutputDir}
OutputBaseFilename=XRTranslate-v{#AppVersion}-win-x64-setup
Compression=lzma2/normal
SolidCompression=yes

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"
Name: "chinesesimplified"; MessagesFile: "compiler:Languages\ChineseSimplified.isl"

[CustomMessages]
english.Location=Install location
english.Open=Open XRTranslate
english.NewerInstalled=A newer version of XRTranslate is already installed.
english.EmptyFolder=Choose an empty folder. This folder belongs to another installation.
english.Complete=Installation complete
chinesesimplified.Location=安装位置
chinesesimplified.Open=打开 XRTranslate
chinesesimplified.NewerInstalled=已安装更新版本的 XRTranslate。
chinesesimplified.EmptyFolder=请选择空文件夹。此文件夹属于另一个安装版本。
chinesesimplified.Complete=安装完成

[Files]
#include "files.iss"

[InstallDelete]
; Refresh bundled plugin directories without touching independently installed ones.
#include "plugins.iss"
Type: files; Name: "{app}\XRTranslate-v*.exe"

[Icons]
Name: "{userprograms}\XRTranslate"; Filename: "{app}\XRTranslate.exe"; WorkingDir: "{app}"

[Run]
Filename: "{app}\XRTranslate.exe"; Description: "{cm:Open}"; Flags: nowait postinstall skipifsilent

[Code]
var
  PreviousVersion, PreviousDirectory: String;
  Heading, VersionLabel: TNewStaticText;
  LocalData: Boolean;

function UseSharedData: Boolean;
begin
  Result := not LocalData;
end;

function InitializeSetup: Boolean;
var
  Installed, Incoming: Int64;
begin
  RegQueryStringValue(HKCU64, 'Software\Microsoft\Windows\CurrentVersion\Uninstall\{#AppId}_is1',
    'DisplayVersion', PreviousVersion);
  RegQueryStringValue(HKCU64, 'Software\Microsoft\Windows\CurrentVersion\Uninstall\{#AppId}_is1',
    'InstallLocation', PreviousDirectory);
  Result := True;
  if StrToVersion(PreviousVersion, Installed) and StrToVersion('{#AppVersion}', Incoming) then
    if ComparePackedVersion(Installed, Incoming) > 0 then begin
      SuppressibleMsgBox(CustomMessage('NewerInstalled'), mbInformation, MB_OK, IDOK);
      Result := False;
    end;
end;

procedure InitializeWizard;
var
  ExtraHeight: Integer;
begin
  WizardForm.PageNameLabel.Hide;
  WizardForm.PageDescriptionLabel.Hide;
  ExtraHeight := ScaleY(72);
  WizardForm.MainPanel.Height := WizardForm.MainPanel.Height + ExtraHeight;
  WizardForm.InnerNotebook.Top := WizardForm.InnerNotebook.Top + ExtraHeight;
  WizardForm.InnerNotebook.Height := WizardForm.InnerNotebook.Height - ExtraHeight;
  WizardForm.WizardSmallBitmapImage.SetBounds(
    (WizardForm.MainPanel.Width - ScaleX(56)) div 2, ScaleY(12), ScaleX(56), ScaleY(56));
  Heading := TNewStaticText.Create(WizardForm);
  Heading.Parent := WizardForm.MainPanel;
  Heading.AutoSize := False;
  Heading.Alignment := taCenter;
  Heading.Font.Size := 18;
  Heading.Caption := 'XRTranslate';
  Heading.SetBounds(0, ScaleY(73), WizardForm.MainPanel.Width, ScaleY(30));
  VersionLabel := TNewStaticText.Create(WizardForm);
  VersionLabel.Parent := WizardForm.MainPanel;
  VersionLabel.AutoSize := False;
  VersionLabel.Alignment := taCenter;
  VersionLabel.Caption := '{#AppVersion}';
  if (PreviousVersion <> '') and (PreviousVersion <> '{#AppVersion}') then
    VersionLabel.Caption := PreviousVersion + '  →  {#AppVersion}';
  VersionLabel.SetBounds(0, ScaleY(106), WizardForm.MainPanel.Width, ScaleY(20));
  WizardForm.SelectDirLabel.Caption := CustomMessage('Location');
  WizardForm.SelectDirBrowseLabel.Hide;
  WizardForm.DiskSpaceLabel.Hide;
  WizardForm.DirEdit.Top := ScaleY(42);
  WizardForm.DirBrowseButton.Top := WizardForm.DirEdit.Top;
  if PreviousVersion <> '' then begin
    WizardForm.DirEdit.ReadOnly := True;
    WizardForm.DirBrowseButton.Hide;
  end;
end;

procedure CurPageChanged(CurPageID: Integer);
begin
  if CurPageID = wpSelectDir then
    WizardForm.NextButton.Caption := SetupMessage(msgButtonInstall);
  if CurPageID = wpFinished then begin
    WizardForm.WizardBitmapImage2.Hide;
    WizardForm.FinishedHeadingLabel.Hide;
    WizardForm.WizardSmallBitmapImage.Parent := WizardForm.FinishedPage;
    Heading.Parent := WizardForm.FinishedPage;
    VersionLabel.Parent := WizardForm.FinishedPage;
    VersionLabel.Caption := '{#AppVersion}';
    WizardForm.FinishedLabel.Caption := CustomMessage('Complete');
    WizardForm.FinishedLabel.Alignment := taCenter;
    WizardForm.FinishedLabel.SetBounds(0, ScaleY(160), WizardForm.FinishedPage.Width, ScaleY(24));
    WizardForm.RunList.SetBounds(ScaleX(160), ScaleY(205), ScaleX(260), ScaleY(40));
  end;
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
var
  Channel: AnsiString;
  Entry: TFindRec;
begin
  Result := '';
  LocalData := not FileExists(ExpandConstant('{app}\distribution.json')) and
    FileExists(ExpandConstant('{app}\release-manifest.json')) and
    FileExists(ExpandConstant('{app}\config.json')) and
    FileExists(ExpandConstant('{app}\bin\xrtranslate-updater.exe')) and
    not FileExists(ExpandConstant('{app}\Cargo.toml')) and
    not DirExists(ExpandConstant('{app}\.git'));
  if LocalData then
    Exit;
  if (PreviousDirectory <> '') and
     (CompareText(RemoveBackslash(PreviousDirectory), RemoveBackslash(ExpandConstant('{app}'))) = 0) and
     LoadStringFromFile(ExpandConstant('{app}\distribution.json'), Channel) and
     (Trim(Channel) = '"installer"') then
    Exit;
  if FindFirst(ExpandConstant('{app}\*'), Entry) then begin
    try
      repeat
        if (Entry.Name <> '.') and (Entry.Name <> '..') then begin
          Result := CustomMessage('EmptyFolder');
          Exit;
        end;
      until not FindNext(Entry);
    finally
      FindClose(Entry);
    end;
  end;
end;
