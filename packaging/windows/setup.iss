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
english.Move=Move…
english.MoveTo=Move installation to
english.Uninstall=Uninstall (keep user data)
english.CloseApp=Exit XRTranslate from the notification area before moving or uninstalling it.
english.UninstallFailed=The previous installation was not removed. Its location remains unchanged.
english.KeepLocation=Upgrade in the existing location, or choose Move.
english.DataError=Could not save the user data location.
english.DataMoveFailed=Could not move user data.
english.DataLocation=User data location
english.MoveData=Move user data
english.MovingData=Moving user data…
chinesesimplified.Location=安装位置
chinesesimplified.Open=打开 XRTranslate
chinesesimplified.NewerInstalled=已安装更新版本的 XRTranslate。
chinesesimplified.EmptyFolder=请选择空文件夹。此文件夹属于另一个安装版本。
chinesesimplified.Complete=安装完成
chinesesimplified.Move=迁移…
chinesesimplified.MoveTo=迁移安装位置
chinesesimplified.Uninstall=卸载（保留用户数据）
chinesesimplified.CloseApp=请先从通知区域退出 XRTranslate，再迁移或卸载。
chinesesimplified.UninstallFailed=旧安装尚未卸载，安装位置保持不变。
chinesesimplified.KeepLocation=请在原位置升级，或选择迁移。
chinesesimplified.DataError=无法保存用户数据位置。
chinesesimplified.DataMoveFailed=无法迁移用户数据。
chinesesimplified.DataLocation=用户数据位置
chinesesimplified.MoveData=迁移用户数据
chinesesimplified.MovingData=正在迁移用户数据…

[Files]
Source: "{#PackageDir}\bin\xrtranslate-updater.exe"; DestDir: "{tmp}"; Flags: dontcopy
Source: "{#PackageDir}\bin\*.dll"; DestDir: "{tmp}"; Flags: dontcopy
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
function CreateDataMoveMutex(Attributes: NativeInt; InitialOwner: Boolean; Name: String): THandle;
  external 'CreateMutexW@kernel32.dll stdcall';
function CloseDataMoveMutex(Handle: THandle): Boolean;
  external 'CloseHandle@kernel32.dll stdcall';

const
  UninstallKey = 'Software\Microsoft\Windows\CurrentVersion\Uninstall\{#AppId}_is1';
var
  PreviousVersion, PreviousDirectory, UninstallCommand: String;
  DataDirectory, MoveDirectory, RetainedDirectory, MoveDataDirectory: String;
  Heading, VersionLabel, DataLabel: TNewStaticText;
  MoveButton, UninstallButton, DataButton: TNewButton;
  DataEdit: TNewEdit;
  DataProgress: TOutputProgressWizardPage;
  DataMoveError: String;
  LocalData: Boolean;

function UseSharedData: Boolean;
begin
  Result := not LocalData;
end;

function SameDirectory(const Left, Right: String): Boolean;
begin
  Result := CompareText(RemoveBackslash(ExpandFileName(Left)),
    RemoveBackslash(ExpandFileName(Right))) = 0;
end;

function SharedDataLocationFile: String;
begin
  Result := ExpandConstant('{localappdata}\XRTranslate\data-directory.txt');
end;

function SaveDataLocation: Boolean;
begin
  Result := ForceDirectories(ExtractFileDir(SharedDataLocationFile)) and
    SaveStringToFile(SharedDataLocationFile, Utf8Encode(DataDirectory), False);
end;

function ReadDataLocation(const LocationFile, DefaultDirectory: String): String;
var
  Contents: AnsiString;
begin
  Result := DefaultDirectory;
  if LoadStringFromFile(LocationFile, Contents) and (Trim(Contents) <> '') then
    Result := Trim(Utf8Decode(Contents));
end;

function EmptyDirectory(const Directory: String): Boolean;
var
  Entry: TFindRec;
begin
  Result := not FileExists(Directory);
  if FindFirst(AddBackslash(Directory) + '*', Entry) then begin
    try
      repeat
        if (Entry.Name <> '.') and (Entry.Name <> '..') then begin
          Result := False;
          Exit;
        end;
      until not FindNext(Entry);
    finally
      FindClose(Entry);
    end;
  end;
end;

function ExistingDataDirectory(const Directory: String): String;
var
  Contents: AnsiString;
begin
  Result := ReadDataLocation(AddBackslash(Directory) + 'data-directory.txt', Directory);
  if LoadStringFromFile(AddBackslash(Directory) + 'distribution.json', Contents) and
     (Trim(Contents) = '"installer"') then begin
    Result := ReadDataLocation(SharedDataLocationFile, ExpandConstant('{localappdata}\XRTranslate'));
  end;
end;

function IsPortableInstallation(const Directory: String): Boolean;
begin
  Result := not FileExists(AddBackslash(Directory) + 'distribution.json') and
    FileExists(AddBackslash(Directory) + 'release-manifest.json') and
    FileExists(AddBackslash(Directory) + 'config.json') and
    FileExists(AddBackslash(Directory) + 'bin\xrtranslate-updater.exe') and
    not FileExists(AddBackslash(Directory) + 'Cargo.toml') and
    not DirExists(AddBackslash(Directory) + '.git');
end;

function InitializeSetup: Boolean;
var
  Installed, Incoming: Int64;
begin
  RegQueryStringValue(HKCU64, UninstallKey, 'DisplayVersion', PreviousVersion);
  RegQueryStringValue(HKCU64, UninstallKey, 'InstallLocation', PreviousDirectory);
  RegQueryStringValue(HKCU64, UninstallKey, 'UninstallString', UninstallCommand);
  DataDirectory := ReadDataLocation(SharedDataLocationFile, ExpandConstant('{localappdata}\XRTranslate'));
  if PreviousDirectory <> '' then
    DataDirectory := ExistingDataDirectory(PreviousDirectory);
  Result := True;
  if StrToVersion(PreviousVersion, Installed) and StrToVersion('{#AppVersion}', Incoming) then
    if ComparePackedVersion(Installed, Incoming) > 0 then begin
      SuppressibleMsgBox(CustomMessage('NewerInstalled'), mbInformation, MB_OK, IDOK);
      Result := False;
    end;
end;

procedure DataLocationChanged(Sender: TObject);
begin
  if PreviousDirectory <> '' then
    DataDirectory := ExistingDataDirectory(PreviousDirectory)
  else begin
    if IsPortableInstallation(WizardForm.DirEdit.Text) then
      DataDirectory := ExistingDataDirectory(WizardForm.DirEdit.Text)
    else if RetainedDirectory = '' then
      DataDirectory := ReadDataLocation(SharedDataLocationFile, ExpandConstant('{localappdata}\XRTranslate'));
  end;
  if MoveDataDirectory <> '' then
    DataEdit.Text := MoveDataDirectory
  else
    DataEdit.Text := DataDirectory;
end;

procedure DataMoveOutput(const Line: String; const Error, FirstLine: Boolean);
var
  Progress: Integer;
begin
  Progress := StrToIntDef(Line, -1);
  if not Error and (Progress >= 0) and (Progress <= 100) then
    DataProgress.SetProgress(Progress, 100)
  else begin
    DataMoveError := DataMoveError + Line + #13#10;
    Log(Line);
  end;
end;

function RunDataMove(CheckOnly: Boolean): Boolean;
var
  Root, LocationFile, Parameters: String;
  ResultCode: Integer;
begin
  Root := WizardForm.DirEdit.Text;
  LocationFile := SharedDataLocationFile;
  if IsPortableInstallation(Root) then LocationFile := AddBackslash(Root) + 'data-directory.txt';
  Parameters := 'move-data --root ' + AddQuotes(RemoveBackslash(Root)) +
    ' --source ' + AddQuotes(RemoveBackslash(DataDirectory)) + ' --target ' + AddQuotes(RemoveBackslash(MoveDataDirectory)) +
    ' --location-file ' + AddQuotes(LocationFile);
  if CheckOnly then Parameters := Parameters + ' --check';
  ExtractTemporaryFiles('{tmp}\*');
  DataMoveError := '';
  Result := ExecAndLogOutput(ExpandConstant('{tmp}\xrtranslate-updater.exe'), Parameters, '',
    SW_HIDE, ewWaitUntilTerminated, ResultCode, @DataMoveOutput);
  if not Result then DataMoveError := SysErrorMessage(ResultCode);
  Result := Result and (ResultCode = 0);
  if not Result then begin
    if DataMoveError = '' then DataMoveError := SysErrorMessage(ResultCode);
    MsgBox(DataMoveError, mbError, MB_OK);
  end;
end;

procedure ChooseDataLocation(Sender: TObject);
var
  Destination: String;
begin
  if MoveDataDirectory <> '' then begin
    MoveDataDirectory := '';
    DataButton.Caption := CustomMessage('Move');
    DataLocationChanged(Sender);
    Exit;
  end;
  DataLocationChanged(Sender);
  Destination := DataDirectory;
  if not BrowseForFolder(CustomMessage('MoveData'), Destination, True) or
     SameDirectory(Destination, DataDirectory) then Exit;
  MoveDataDirectory := Destination;
  if not RunDataMove(True) then begin
    MoveDataDirectory := '';
    Exit;
  end;
  DataEdit.Text := Destination;
  DataButton.Caption := SetupMessage(msgButtonCancel);
end;

procedure MoveUserData;
var
  MoveMutex: THandle;
begin
  MoveMutex := CreateDataMoveMutex(0, False, 'Local\XRTranslate.Desktop');
  if MoveMutex = 0 then begin
    MsgBox(SysErrorMessage(DLLGetLastError), mbError, MB_OK);
    Exit;
  end;
  if DLLGetLastError = 183 then begin
    CloseDataMoveMutex(MoveMutex);
    MsgBox(CustomMessage('CloseApp'), mbInformation, MB_OK);
    Exit;
  end;
  try
    DataProgress.SetText(CustomMessage('MovingData'), MoveDataDirectory);
    DataProgress.SetProgress(0, 100);
    DataProgress.Show;
    if not RunDataMove(False) then Exit;
    DataDirectory := MoveDataDirectory;
    MoveDataDirectory := '';
    DataEdit.Text := DataDirectory;
  finally
    DataProgress.Hide;
    CloseDataMoveMutex(MoveMutex);
  end;
end;

function UninstallPrevious: String;
var
  ResultCode: Integer;
begin
  Result := CustomMessage('CloseApp');
  if CheckForMutexes('Local\XRTranslate.Desktop') then
    Exit;
  Result := CustomMessage('UninstallFailed');
  if UninstallCommand = '' then
    Exit;
  DataLocationChanged(WizardForm);
  if not SaveDataLocation then begin
    Result := CustomMessage('DataError');
    Exit;
  end;
  if not Exec('>', UninstallCommand + ' /NORESTART', '', SW_SHOWNORMAL,
     ewWaitUntilTerminated, ResultCode) then begin
    Result := SysErrorMessage(ResultCode);
    Exit;
  end;
  if (ResultCode <> 0) or RegKeyExists(HKCU64, UninstallKey) then
    Exit;
  RetainedDirectory := PreviousDirectory;
  PreviousDirectory := '';
  PreviousVersion := '';
  UninstallCommand := '';
  Result := '';
end;

procedure MoveInstallation(Sender: TObject);
var
  Destination: String;
begin
  if MoveDirectory <> '' then begin
    MoveDirectory := '';
    WizardForm.DirEdit.Text := PreviousDirectory;
    MoveButton.Caption := CustomMessage('Move');
    Exit;
  end;
  Destination := PreviousDirectory;
  if not BrowseForFolder(CustomMessage('MoveTo'), Destination, True) or
     SameDirectory(Destination, PreviousDirectory) then
    Exit;
  if not EmptyDirectory(Destination) or
     (Pos(Uppercase(AddBackslash(PreviousDirectory)), Uppercase(AddBackslash(Destination))) = 1) then begin
    MsgBox(CustomMessage('EmptyFolder'), mbError, MB_OK);
    Exit;
  end;
  MoveDirectory := Destination;
  WizardForm.DirEdit.Text := Destination;
  MoveButton.Caption := SetupMessage(msgButtonCancel);
end;

procedure UninstallInstallation(Sender: TObject);
var
  Error: String;
begin
  Error := UninstallPrevious;
  if Error <> '' then begin
    MsgBox(Error, mbError, MB_OK);
    Exit;
  end;
  MoveDirectory := '';
  WizardForm.DirEdit.ReadOnly := False;
  WizardForm.DirBrowseButton.Show;
  MoveButton.Hide;
  UninstallButton.Hide;
  VersionLabel.Caption := '{#AppVersion}';
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
  DataLabel := TNewStaticText.Create(WizardForm);
  DataLabel.Parent := WizardForm.DirEdit.Parent;
  DataLabel.Caption := CustomMessage('DataLocation');
  DataLabel.SetBounds(WizardForm.DirEdit.Left, ScaleY(82), WizardForm.DirEdit.Width, ScaleY(20));
  DataEdit := TNewEdit.Create(WizardForm);
  DataEdit.Parent := WizardForm.DirEdit.Parent;
  DataEdit.ReadOnly := True;
  DataEdit.SetBounds(WizardForm.DirEdit.Left, ScaleY(106), WizardForm.DirEdit.Width, WizardForm.DirEdit.Height);
  DataButton := TNewButton.Create(WizardForm);
  DataButton.Parent := WizardForm.DirBrowseButton.Parent;
  DataButton.SetBounds(WizardForm.DirBrowseButton.Left, DataEdit.Top,
    WizardForm.DirBrowseButton.Width, WizardForm.DirBrowseButton.Height);
  DataButton.Caption := CustomMessage('Move');
  DataButton.OnClick := @ChooseDataLocation;
  DataProgress := CreateOutputProgressPage(CustomMessage('MovingData'), '');
  DataLocationChanged(WizardForm);
  WizardForm.DirEdit.OnChange := @DataLocationChanged;
  if PreviousDirectory <> '' then begin
    // UsePreviousAppDir supplies only a default; /DIR must not create a second copy.
    WizardForm.DirEdit.Text := PreviousDirectory;
    WizardForm.DirEdit.ReadOnly := True;
    WizardForm.DirBrowseButton.Hide;
    MoveButton := TNewButton.Create(WizardForm);
    MoveButton.Parent := WizardForm.DirBrowseButton.Parent;
    MoveButton.SetBounds(WizardForm.DirBrowseButton.Left, WizardForm.DirBrowseButton.Top,
      WizardForm.DirBrowseButton.Width, WizardForm.DirBrowseButton.Height);
    MoveButton.Caption := CustomMessage('Move');
    MoveButton.OnClick := @MoveInstallation;
    UninstallButton := TNewButton.Create(WizardForm);
    UninstallButton.Parent := WizardForm.DirBrowseButton.Parent;
    UninstallButton.SetBounds(WizardForm.DirEdit.Left, ScaleY(138),
      ScaleX(205), WizardForm.DirBrowseButton.Height);
    UninstallButton.Caption := CustomMessage('Uninstall');
    UninstallButton.OnClick := @UninstallInstallation;
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

procedure CurStepChanged(CurStep: TSetupStep);
begin
  // Upgrade first so cancelling Setup cannot strand data from an older client.
  if (CurStep = ssPostInstall) and (MoveDataDirectory <> '') then
    MoveUserData;
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
var
  Destination: String;
begin
  Result := '';
  DataLocationChanged(WizardForm);
  if (MoveDataDirectory <> '') and not RunDataMove(True) then begin
    Result := CustomMessage('DataMoveFailed');
    Exit;
  end;
  Destination := ExpandConstant('{app}');
  if PreviousDirectory <> '' then begin
    if MoveDirectory = '' then begin
      if not SameDirectory(Destination, PreviousDirectory) then begin
        Result := CustomMessage('KeepLocation');
        Exit;
      end;
    end else begin
      if not SameDirectory(Destination, MoveDirectory) or not EmptyDirectory(Destination) then begin
        Result := CustomMessage('EmptyFolder');
        Exit;
      end;
      Result := UninstallPrevious;
      if Result <> '' then
        Exit;
      MoveDirectory := '';
    end;
  end;
  LocalData := IsPortableInstallation(Destination);
  if (RetainedDirectory <> '') and SameDirectory(Destination, RetainedDirectory) then
    LocalData := SameDirectory(Destination, DataDirectory);
  if LocalData then
    Exit;
  if not ((PreviousDirectory <> '') and SameDirectory(Destination, PreviousDirectory)) and
     not ((RetainedDirectory <> '') and SameDirectory(Destination, RetainedDirectory)) and
     not EmptyDirectory(Destination) then begin
    Result := CustomMessage('EmptyFolder');
    Exit;
  end;
  if not SaveDataLocation then
    Result := CustomMessage('DataError');
end;

function InitializeUninstall: Boolean;
begin
  DataDirectory := ExistingDataDirectory(ExpandConstant('{app}'));
  Result := SaveDataLocation;
  if not Result then
    SuppressibleMsgBox(CustomMessage('DataError'), mbError, MB_OK, IDOK);
end;
