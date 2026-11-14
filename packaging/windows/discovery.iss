// Registration is authoritative; nearby discovery is only for unregistered ZIPs.
const
  SearchDirectoryLimit = 256;
  SearchEntryLimit = 4096;
  SearchTimeMs = 750;

type
  TSearchDirectory = record
    Path: String;
    Depth: Integer;
  end;
  TSearchQueue = array of TSearchDirectory;

function SearchClock: Int64;
  external 'GetTickCount64@kernel32.dll stdcall';
function SearchDriveType(Root: String): Cardinal;
  external 'GetDriveTypeW@kernel32.dll stdcall';
function SearchAttributes(Path: String): Cardinal;
  external 'GetFileAttributesW@kernel32.dll stdcall';

function IsDevelopmentDirectory(const Directory: String): Boolean;
begin
  Result := FileExists(AddBackslash(Directory) + 'Cargo.toml') or
    FileExists(AddBackslash(Directory) + '.git') or
    DirExists(AddBackslash(Directory) + '.git');
end;

function IsPortableInstallation(const Directory: String): Boolean;
begin
  Result := (Directory <> '') and DirExists(Directory) and
    not FileExists(AddBackslash(Directory) + 'distribution.json') and
    FileExists(AddBackslash(Directory) + 'release-manifest.json') and
    FileExists(AddBackslash(Directory) + 'config.json') and
    FileExists(AddBackslash(Directory) + 'bin\xrtranslate-updater.exe') and
    not IsDevelopmentDirectory(Directory);
end;

function PortableVersion(const Directory: String): String;
var
  Entry: TFindRec;
  Version: String;
  Candidate, Latest: Int64;
begin
  Result := '';
  Latest := 0;
  if GetVersionNumbersString(AddBackslash(Directory) + 'XRTranslate.exe', Version) and
     StrToVersion(Version, Candidate) then begin
    Latest := Candidate;
    Result := Version;
    if Copy(Result, Length(Result) - 1, 2) = '.0' then
      Delete(Result, Length(Result) - 1, 2);
  end;
  if FindFirst(AddBackslash(Directory) + 'XRTranslate-v*.exe', Entry) then begin
    try
      repeat
        Version := Copy(Entry.Name, 14, Length(Entry.Name) - 17);
        if (Entry.Attributes and FILE_ATTRIBUTE_DIRECTORY = 0) and
           StrToVersion(Version, Candidate) and (Candidate > Latest) then begin
          Latest := Candidate;
          Result := Version;
        end;
      until not FindNext(Entry);
    finally
      FindClose(Entry);
    end;
  end;
end;

function LatestInstallationIndex(const Directories: TArrayOfString): Integer;
var
  Index: Integer;
  Version, Latest: Int64;
begin
  Result := -1;
  Latest := 0;
  for Index := 0 to GetArrayLength(Directories) - 1 do
    if StrToVersion(PortableVersion(Directories[Index]), Version) then
      if (Result < 0) or (ComparePackedVersion(Version, Latest) > 0) then begin
        Result := Index;
        Latest := Version;
      end;
end;

procedure AddSearchDirectory(var Queue: TSearchQueue; const Directory: String; Depth: Integer);
var
  Path: String;
  Index, Count: Integer;
  Attributes, Drive: Cardinal;
begin
  Count := GetArrayLength(Queue);
  if (Directory = '') or (Count >= SearchDirectoryLimit) then Exit;
  Path := ExpandFileName(Directory);
  // Network shares and directory links can make a small search unexpectedly slow.
  if (Length(Path) < 3) or (Path[2] <> ':') then Exit;
  Drive := SearchDriveType(Copy(Path, 1, 3));
  if (Drive <> 2) and (Drive <> 3) then Exit;
  Attributes := SearchAttributes(Path);
  if (Attributes = $FFFFFFFF) or (Attributes and FILE_ATTRIBUTE_DIRECTORY = 0) or
     (Attributes and (FILE_ATTRIBUTE_REPARSE_POINT or FILE_ATTRIBUTE_HIDDEN or
       FILE_ATTRIBUTE_SYSTEM or FILE_ATTRIBUTE_OFFLINE) <> 0) then Exit;
  for Index := 0 to Count - 1 do
    if SameDirectory(Queue[Index].Path, Path) then Exit;
  SetArrayLength(Queue, Count + 1);
  Queue[Count].Path := Path;
  Queue[Count].Depth := Depth;
end;

procedure FindNearbyInstallations(const SourceDirectory: String; var Found: TArrayOfString);
var
  Queue: TSearchQueue;
  Entry: TFindRec;
  Root, Parent: String;
  Started: Int64;
  Index, Level, Count, Entries: Integer;
begin
  SetArrayLength(Found, 0);
  Started := SearchClock;
  Root := ExpandFileName(SourceDirectory);
  // Each root is searched breadth-first, at most two child levels deep.
  for Level := 0 to 2 do begin
    if IsDevelopmentDirectory(Root) then Break;
    AddSearchDirectory(Queue, Root, 0);
    Parent := ExtractFileDir(RemoveBackslash(Root));
    if (Length(Root) <= 3) or (Parent = '') or SameDirectory(Root, Parent) or
       SameDirectory(Root, GetEnv('USERPROFILE')) then Break;
    Root := Parent;
  end;
  Index := 0;
  Entries := 0;
  while (Index < GetArrayLength(Queue)) and (Entries < SearchEntryLimit) and
        (SearchClock - Started < SearchTimeMs) do begin
    Root := Queue[Index].Path;
    Level := Queue[Index].Depth;
    Index := Index + 1;
    if IsDevelopmentDirectory(Root) then Continue;
    if IsPortableInstallation(Root) and (PortableVersion(Root) <> '') then begin
      Count := GetArrayLength(Found);
      SetArrayLength(Found, Count + 1);
      Found[Count] := Root;
      // A client's models, plugins and user data are never search roots.
      Continue;
    end;
    if Level >= 2 then Continue;
    if FindFirst(AddBackslash(Root) + '*', Entry) then begin
      try
        repeat
          Entries := Entries + 1;
          if (Entries >= SearchEntryLimit) or (SearchClock - Started >= SearchTimeMs) then Break;
          if (Entry.Name <> '.') and (Entry.Name <> '..') and
             (Entry.Attributes and FILE_ATTRIBUTE_DIRECTORY <> 0) then
            AddSearchDirectory(Queue, AddBackslash(Root) + Entry.Name, Level + 1);
        until not FindNext(Entry);
      finally
        FindClose(Entry);
      end;
    end;
  end;
  Log(Format('Nearby installations: %d found, %d directories, %d entries, %d ms', [
    GetArrayLength(Found), Index, Entries, SearchClock - Started]));
end;
