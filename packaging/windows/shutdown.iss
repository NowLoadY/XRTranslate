// Restart Manager targets this installation, including older versioned clients.
function RmStartSession(var Session: DWORD; Flags: DWORD; SessionKey: String): DWORD;
  external 'RmStartSession@rstrtmgr.dll stdcall';
function RmRegisterResources(Session: DWORD; FileCount: UINT; const FileNames: TArrayOfString;
  AppCount: UINT; Apps: NativeInt; ServiceCount: UINT; Services: NativeInt): DWORD;
  external 'RmRegisterResources@rstrtmgr.dll stdcall';
function RmShutdown(Session: DWORD; Flags: ULONG; Callback: NativeInt): DWORD;
  external 'RmShutdown@rstrtmgr.dll stdcall';
function RmEndSession(Session: DWORD): DWORD;
  external 'RmEndSession@rstrtmgr.dll stdcall';

function CloseInstalledApplication(const Directory: String): Boolean;
var
  Files: TArrayOfString;
  Entry: TFindRec;
  Version: Int64;
  Session, Status: DWORD;
  Key: String;
  Count: Integer;
begin
  SetArrayLength(Files, 0);
  if FindFirst(AddBackslash(Directory) + 'XRTranslate*.exe', Entry) then begin
    try
      repeat
        if (CompareText(Entry.Name, 'XRTranslate.exe') = 0) or
           ((CompareText(Copy(Entry.Name, 1, 13), 'XRTranslate-v') = 0) and
            StrToVersion(Copy(Entry.Name, 14, Length(Entry.Name) - 17), Version)) then begin
          Count := GetArrayLength(Files);
          SetArrayLength(Files, Count + 1);
          Files[Count] := AddBackslash(Directory) + Entry.Name;
        end;
      until not FindNext(Entry);
    finally
      FindClose(Entry);
    end;
  end;
  Result := True;
  if GetArrayLength(Files) = 0 then Exit;
  SetLength(Key, 33);
  Status := RmStartSession(Session, 0, Key);
  if Status = 0 then begin
    try
      Status := RmRegisterResources(Session, GetArrayLength(Files), Files, 0, 0, 0, 0);
      // Never force termination: the host saves state and waits for its plugins.
      if Status = 0 then Status := RmShutdown(Session, 0, 0);
    finally
      RmEndSession(Session);
    end;
  end;
  Result := Status = 0;
  if not Result then Log('Application shutdown failed: ' + SysErrorMessage(Status));
end;
