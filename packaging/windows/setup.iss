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

[Messages]
english.ConfirmUninstall=Uninstall %1? User data will be kept.
chinesesimplified.ConfirmUninstall=卸载 %1？用户数据会保留。

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
english.ModelsAndData=Models & data
english.AvailableSpace=%1 GB free of %2 GB
english.RestoreLocation=Reset
english.MovingData=Moving user data…
english.SeparateData=Choose a location outside the installation folder.
english.OccupiedData=XRTranslate-UserData already contains files. Choose another location.
english.NestedData=Choose a location outside the current user data folder.
english.OtherInstallation=XRTranslate is already installed in another folder. Update or uninstall that installation first.
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
chinesesimplified.ModelsAndData=模型与数据
chinesesimplified.AvailableSpace=%1 GB 可用 / %2 GB
chinesesimplified.RestoreLocation=重置
chinesesimplified.MovingData=正在迁移用户数据…
chinesesimplified.SeparateData=请选择安装目录以外的位置。
chinesesimplified.OccupiedData=XRTranslate-UserData 已有文件，请选择其他位置。
chinesesimplified.NestedData=请选择当前用户数据目录以外的位置。
chinesesimplified.OtherInstallation=XRTranslate 已安装在另一个位置。请先更新或卸载该安装。

[Files]
; A pre-rendered SVG keeps Setup independent of an extra image renderer.
Source: "model-storage.png"; DestDir: "{tmp}"; Flags: dontcopy
Source: "{#PackageDir}\bin\xrtranslate-updater.exe"; DestDir: "{tmp}"; Flags: dontcopy
Source: "{#PackageDir}\bin\*.dll"; DestDir: "{tmp}"; Flags: dontcopy
Source: "{#InventoryFile}"; DestDir: "{tmp}"; Flags: dontcopy
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
#include "shutdown.iss"

function CreateAppMutex(Attributes: NativeInt; InitialOwner: Boolean; Name: String): THandle;
  external 'CreateMutexW@kernel32.dll stdcall';
function CloseAppMutex(Handle: THandle): Boolean;
  external 'CloseHandle@kernel32.dll stdcall';

const
  UninstallKey = 'Software\Microsoft\Windows\CurrentVersion\Uninstall\{#AppId}_is1';
var
  PreviousVersion, PreviousDirectory, UninstallCommand: String;
  DataDirectory, MoveDirectory, RetainedDirectory, MoveDataDirectory: String;
  Heading, VersionLabel, DataLabel, SpaceLabel: TNewStaticText;
  MoveButton, UninstallButton, DataButton: TNewButton;
  DataEdit: TNewEdit;
  DataPage: TWizardPage;
  DataRestore: TNewLinkLabel;
  DataSpace: TBitmapImage;
  DataProgress: TOutputProgressWizardPage;
  DataMoveMessage: String;
  LocalData, NewData, DataLocationChosen: Boolean;
  UninstallMutex: THandle;

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
  Result := (Directory <> '') and DirExists(Directory) and
    not FileExists(AddBackslash(Directory) + 'distribution.json') and
    FileExists(AddBackslash(Directory) + 'release-manifest.json') and
    FileExists(AddBackslash(Directory) + 'config.json') and
    FileExists(AddBackslash(Directory) + 'bin\xrtranslate-updater.exe') and
    not FileExists(AddBackslash(Directory) + 'Cargo.toml') and
    not DirExists(AddBackslash(Directory) + '.git');
end;

function PortableVersion(const Directory: String): String;
var
  Entry: TFindRec;
  Version: String;
  Candidate, Latest: Int64;
begin
  Result := '';
  Latest := 0;
  if FindFirst(AddBackslash(Directory) + 'XRTranslate-v*.exe', Entry) then begin
    try
      repeat
        Version := Copy(Entry.Name, 14, Length(Entry.Name) - 17);
        if StrToVersion(Version, Candidate) and (Candidate > Latest) then begin
          Latest := Candidate;
          Result := Version;
        end;
      until not FindNext(Entry);
    finally
      FindClose(Entry);
    end;
  end;
end;

function InitializeSetup: Boolean;
var
  Installed, Incoming: Int64;
  RequestedDirectory: String;
begin
  RegQueryStringValue(HKCU64, UninstallKey, 'DisplayVersion', PreviousVersion);
  RegQueryStringValue(HKCU64, UninstallKey, 'InstallLocation', PreviousDirectory);
  RegQueryStringValue(HKCU64, UninstallKey, 'UninstallString', UninstallCommand);
  // Both older clients and the compatibility ZIP pass their actual directory.
  RequestedDirectory := ExpandConstant('{param:DIR|}');
  if IsPortableInstallation(RequestedDirectory) then begin
    if (PreviousDirectory <> '') and not SameDirectory(PreviousDirectory, RequestedDirectory) then begin
      SuppressibleMsgBox(CustomMessage('OtherInstallation'), mbInformation, MB_OK, IDOK);
      Result := False;
      Exit;
    end;
    if PreviousDirectory = '' then begin
      PreviousDirectory := ExpandFileName(RequestedDirectory);
      PreviousVersion := PortableVersion(PreviousDirectory);
    end;
  end;
  DataDirectory := ReadDataLocation(SharedDataLocationFile, ExpandConstant('{localappdata}\XRTranslate'));
  if PreviousDirectory <> '' then
    DataDirectory := ExistingDataDirectory(PreviousDirectory);
  NewData := (PreviousDirectory = '') and not FileExists(SharedDataLocationFile) and EmptyDirectory(DataDirectory);
  Result := True;
  if StrToVersion(PreviousVersion, Installed) and StrToVersion('{#AppVersion}', Incoming) then
    if ComparePackedVersion(Installed, Incoming) > 0 then begin
      SuppressibleMsgBox(CustomMessage('NewerInstalled'), mbInformation, MB_OK, IDOK);
      Result := False;
    end;
end;

function ExistingParentDirectory(const Path: String): String;
var
  Parent: String;
begin
  Result := Path;
  while not DirExists(Result) do begin
    Parent := ExtractFileDir(RemoveBackslash(Result));
    if (Parent = '') or (Parent = Result) then Break;
    Result := Parent;
  end;
end;

procedure ShowDataSpace;
var
  FreeBytes, TotalBytes: Int64;
  BarWidth, BarHeight, UsedWidth: Integer;
  FillColor: TColor;
begin
  SpaceLabel.Caption := '';
  DataSpace.Hide;
  if not GetSpaceOnDisk64(ExistingParentDirectory(DataEdit.Text), FreeBytes, TotalBytes) or (TotalBytes <= 0) then Exit;
  SpaceLabel.Caption := ExtractFileDrive(DataEdit.Text) + '  ·  ' + FmtMessage(CustomMessage('AvailableSpace'), [
    Format('%.1f', [FreeBytes / 1073741824.0]), Format('%.1f', [TotalBytes / 1073741824.0])]);
  FillColor := $00F77E00;
  SpaceLabel.Font.Color := clGrayText;
  if FreeBytes < TotalBytes div 20 then begin
    FillColor := $005A5FB3;
    SpaceLabel.Font.Color := FillColor;
  end;
  // Supersampling keeps the static rounded ends smooth at any display scale.
  BarWidth := DataSpace.Width * 4;
  BarHeight := DataSpace.Height * 4;
  UsedWidth := BarWidth - Round(FreeBytes * 1.0 / TotalBytes * BarWidth);
  with DataSpace.Bitmap do begin
    Width := BarWidth;
    Height := BarHeight;
    Canvas.Pen.Style := psClear;
    Canvas.Brush.Color := clWhite;
    Canvas.Rectangle(0, 0, BarWidth, BarHeight);
    Canvas.Brush.Color := $00FDF0E5;
    Canvas.RoundRect(0, 0, BarWidth, BarHeight, BarHeight, BarHeight);
    Canvas.Brush.Color := FillColor;
    if UsedWidth > 0 then
      Canvas.RoundRect(0, 0, UsedWidth, BarHeight, BarHeight, BarHeight);
  end;
  DataSpace.Show;
end;

procedure DataLocationChanged(Sender: TObject);
var
  RecommendLocation: Boolean;
begin
  if PreviousDirectory <> '' then
    DataDirectory := ExistingDataDirectory(PreviousDirectory)
  else begin
    if IsPortableInstallation(WizardForm.DirEdit.Text) then
      DataDirectory := ExistingDataDirectory(WizardForm.DirEdit.Text)
    else if RetainedDirectory = '' then
      DataDirectory := ReadDataLocation(SharedDataLocationFile, ExpandConstant('{localappdata}\XRTranslate'));
  end;
  RecommendLocation := NewData and (PreviousDirectory = '') and (RetainedDirectory = '') and
    not IsPortableInstallation(WizardForm.DirEdit.Text);
  if not DataLocationChosen then begin
    MoveDataDirectory := '';
    if RecommendLocation then
      MoveDataDirectory := AddBackslash(ExtractFileDir(RemoveBackslash(WizardForm.DirEdit.Text))) + 'XRTranslate-UserData';
  end;
  if MoveDataDirectory <> '' then
    DataEdit.Text := MoveDataDirectory
  else
    DataEdit.Text := DataDirectory;
  DataEdit.Hint := DataEdit.Text;
  DataEdit.SelStart := 0;
  DataRestore.Visible := DataLocationChosen and ((MoveDataDirectory <> '') or RecommendLocation);
  ShowDataSpace;
end;

procedure DataMoveOutput(const Line: String; const Error, FirstLine: Boolean);
var
  Progress: Integer;
begin
  Progress := StrToIntDef(Line, -1);
  if not Error and (Progress >= 0) and (Progress <= 100) then
    DataProgress.SetProgress(Progress, 100)
  else begin
    DataMoveMessage := DataMoveMessage + Line + #13#10;
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
  // A final dot keeps drive roots absolute without a backslash escaping the quote.
  Parameters := 'move-data --root ' + AddQuotes(AddBackslash(Root) + '.') +
    ' --source ' + AddQuotes(AddBackslash(DataDirectory) + '.') + ' --target ' + AddQuotes(AddBackslash(MoveDataDirectory) + '.') +
    ' --location-file ' + AddQuotes(LocationFile);
  if CheckOnly then Parameters := Parameters + ' --check';
  // These entries precede the payload in the solid archive. Extract once only.
  if not FileExists(ExpandConstant('{tmp}\xrtranslate-updater.exe')) then begin
    ExtractTemporaryFiles('{tmp}\*.dll');
    ExtractTemporaryFile('xrtranslate-updater.exe');
  end;
  DataMoveMessage := '';
  Result := ExecAndLogOutput(ExpandConstant('{tmp}\xrtranslate-updater.exe'), Parameters, '',
    SW_HIDE, ewWaitUntilTerminated, ResultCode, @DataMoveOutput);
  if not Result then DataMoveMessage := SysErrorMessage(ResultCode);
  Result := Result and (ResultCode = 0);
  if Result and CheckOnly then MoveDataDirectory := Trim(DataMoveMessage);
  if not Result then begin
    if DataMoveMessage = '' then DataMoveMessage := SysErrorMessage(ResultCode);
    StringChangeEx(DataMoveMessage, 'Choose a data folder outside the installation folder.', CustomMessage('SeparateData'), True);
    StringChangeEx(DataMoveMessage, 'The destination data folder is not empty. Choose another location.', CustomMessage('OccupiedData'), True);
    StringChangeEx(DataMoveMessage, 'Choose a separate, empty data folder.', CustomMessage('NestedData'), True);
    SuppressibleMsgBox(DataMoveMessage, mbError, MB_OK, IDOK);
  end;
end;

procedure ChooseDataLocation(Sender: TObject);
var
  Destination, PreviousTarget: String;
begin
  DataLocationChanged(Sender);
  Destination := ExistingParentDirectory(DataEdit.Text);
  if not BrowseForFolder(CustomMessage('DataLocation'), Destination, True) then Exit;
  if SameDirectory(Destination, DataDirectory) then begin
    MoveDataDirectory := '';
    DataLocationChosen := True;
    DataLocationChanged(Sender);
    Exit;
  end;
  PreviousTarget := MoveDataDirectory;
  MoveDataDirectory := Destination;
  if RunDataMove(True) then
    DataLocationChosen := True
  else
    MoveDataDirectory := PreviousTarget;
  DataLocationChanged(Sender);
end;

procedure RestoreDataLocation(Sender: TObject; const Link: String; LinkType: TSysLinkType);
begin
  DataLocationChosen := False;
  MoveDataDirectory := '';
  DataLocationChanged(Sender);
end;

procedure MoveUserData;
var
  MoveMutex: THandle;
begin
  MoveMutex := CreateAppMutex(0, False, 'Local\XRTranslate.Desktop');
  if MoveMutex = 0 then begin
    MsgBox(SysErrorMessage(DLLGetLastError), mbError, MB_OK);
    Exit;
  end;
  if DLLGetLastError = 183 then begin
    CloseAppMutex(MoveMutex);
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
    CloseAppMutex(MoveMutex);
  end;
end;

function RemovePortableFile(const Relative: String): Boolean;
var
  Filename, Directory: String;
begin
  Filename := AddBackslash(PreviousDirectory) + Relative;
  Result := not FileExists(Filename) or DeleteFile(Filename);
  if not Result then Exit;
  Directory := ExtractFileDir(Filename);
  while not SameDirectory(Directory, PreviousDirectory) do begin
    if not RemoveDir(Directory) then Break;
    Directory := ExtractFileDir(Directory);
  end;
end;

function RemovePortableInstallation: Boolean;
var
  Files: TArrayOfString;
  Entry: TFindRec;
  Index: Integer;
  Version: Int64;
begin
  Result := False;
  if not IsPortableInstallation(PreviousDirectory) then Exit;
  ExtractTemporaryFile('portable-files.txt');
  if not LoadStringsFromFile(ExpandConstant('{tmp}\portable-files.txt'), Files) then Exit;
  // Stop on a locked client before removing any of its other files.
  if FindFirst(AddBackslash(PreviousDirectory) + 'XRTranslate-v*.exe', Entry) then begin
    try
      repeat
        if StrToVersion(Copy(Entry.Name, 14, Length(Entry.Name) - 17), Version) and
           not RemovePortableFile(Entry.Name) then Exit;
      until not FindNext(Entry);
    finally
      FindClose(Entry);
    end;
  end;
  for Index := 0 to GetArrayLength(Files) - 1 do
    if not RemovePortableFile(Files[Index]) then Exit;
  Result := RemovePortableFile('release-manifest.json');
end;

function UninstallPrevious: String;
var
  ResultCode: Integer;
begin
  Result := CustomMessage('CloseApp');
  if not CloseInstalledApplication(PreviousDirectory) then Exit;
  Result := CustomMessage('UninstallFailed');
  DataLocationChanged(WizardForm);
  if not SaveDataLocation then begin
    Result := CustomMessage('DataError');
    Exit;
  end;
  if UninstallCommand = '' then begin
    if not RemovePortableInstallation then Exit;
  end else begin
    if not Exec('>', UninstallCommand + ' /SILENT /SUPPRESSMSGBOXES /NORESTART', '', SW_SHOWNORMAL,
       ewWaitUntilTerminated, ResultCode) then begin
      Result := SysErrorMessage(ResultCode);
      Exit;
    end;
    if (ResultCode <> 0) or RegKeyExists(HKCU64, UninstallKey) then Exit;
  end;
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
  if SuppressibleMsgBox(FmtMessage(SetupMessage(msgConfirmUninstall), ['XRTranslate']),
    mbConfirmation, MB_YESNO, IDNO) <> IDYES then Exit;
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
  DataIcon: TBitmapImage;
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

  DataPage := CreateCustomPage(wpSelectDir, CustomMessage('ModelsAndData'), '');
  ExtractTemporaryFile('model-storage.png');
  DataIcon := TBitmapImage.Create(DataPage);
  DataIcon.Parent := DataPage.Surface;
  DataIcon.BackColor := clNone;
  DataIcon.Stretch := True;
  DataIcon.PngImage.LoadFromFile(ExpandConstant('{tmp}\model-storage.png'));
  DataIcon.SetBounds(0, ScaleY(8), ScaleX(32), ScaleY(32));
  DataLabel := TNewStaticText.Create(DataPage);
  DataLabel.Parent := DataPage.Surface;
  DataLabel.Caption := CustomMessage('ModelsAndData');
  DataLabel.ShowAccelChar := False;
  DataLabel.Font.Size := 12;
  DataLabel.SetBounds(ScaleX(44), ScaleY(13), DataPage.SurfaceWidth - ScaleX(44), ScaleY(26));
  DataEdit := TNewEdit.Create(DataPage);
  DataEdit.Parent := DataPage.Surface;
  DataEdit.ReadOnly := True;
  DataEdit.AutoSelect := False;
  DataEdit.ShowHint := True;
  DataEdit.SetBounds(0, ScaleY(58), WizardForm.DirEdit.Width, WizardForm.DirEdit.Height);
  DataButton := TNewButton.Create(DataPage);
  DataButton.Parent := DataPage.Surface;
  DataButton.SetBounds(WizardForm.DirBrowseButton.Left, DataEdit.Top,
    WizardForm.DirBrowseButton.Width, WizardForm.DirBrowseButton.Height);
  DataButton.Caption := SetupMessage(msgButtonBrowse);
  DataButton.OnClick := @ChooseDataLocation;
  SpaceLabel := TNewStaticText.Create(DataPage);
  SpaceLabel.Parent := DataPage.Surface;
  SpaceLabel.SetBounds(0, ScaleY(101), DataPage.SurfaceWidth, ScaleY(20));
  DataSpace := TBitmapImage.Create(DataPage);
  DataSpace.Parent := DataPage.Surface;
  DataSpace.BackColor := clNone;
  DataSpace.Stretch := True;
  DataSpace.SetBounds(0, ScaleY(129), DataPage.SurfaceWidth, ScaleY(4));
  DataRestore := TNewLinkLabel.Create(DataPage);
  DataRestore.Parent := DataPage.Surface;
  DataRestore.Caption := '<a id="restore">' + CustomMessage('RestoreLocation') + '</a>';
  DataRestore.SetBounds(0, ScaleY(153), DataPage.SurfaceWidth, ScaleY(20));
  DataRestore.OnLinkClick := @RestoreDataLocation;
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
    WizardForm.NextButton.Caption := SetupMessage(msgButtonNext);
  if CurPageID = DataPage.ID then begin
    DataLocationChanged(WizardForm);
    WizardForm.NextButton.Caption := SetupMessage(msgButtonInstall);
  end;
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
  // A completed uninstall can leave this directory containing only user data.
  if (PreviousDirectory = '') and FileExists(SharedDataLocationFile) and
     SameDirectory(ExistingDataDirectory(Destination), DataDirectory) and
     not FileExists(AddBackslash(Destination) + 'release-manifest.json') and
     not FileExists(AddBackslash(Destination) + 'Cargo.toml') and
     not DirExists(AddBackslash(Destination) + '.git') then
    LocalData := True;
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

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  Directory: String;
begin
  // This event follows confirmation and precedes every file deletion.
  if CurUninstallStep <> usUninstall then Exit;
  Directory := ExpandConstant('{app}');
  if not CloseInstalledApplication(Directory) then
    RaiseException(CustomMessage('CloseApp'));
  UninstallMutex := CreateAppMutex(0, False, 'Local\XRTranslate.Desktop');
  if UninstallMutex = 0 then RaiseException(SysErrorMessage(DLLGetLastError));
  DataDirectory := ExistingDataDirectory(Directory);
  if not SaveDataLocation then RaiseException(CustomMessage('DataError'));
  // Retain the location of data nested inside the program folder for reinstall.
  if not SameDirectory(Directory, DataDirectory) and
     (Pos(Uppercase(AddBackslash(Directory)), Uppercase(AddBackslash(DataDirectory))) = 1) then
    if not SaveStringToFile(AddBackslash(Directory) + 'data-directory.txt', Utf8Encode(DataDirectory), False) then
      RaiseException(CustomMessage('DataError'));
end;

procedure DeinitializeUninstall;
begin
  if UninstallMutex <> 0 then CloseAppMutex(UninstallMutex);
end;
