// Shared installed-profile detection for offline and web installers.
function InstalledProfile: String;
begin
  Result := '';
  if RegKeyExists(HKCU64, PublicUninstallKey) then begin
    // The original public installer contains the complete payload, without a marker.
    Result := GetPreviousData('ContentProfile', 'complete');
    if (Result <> 'minimal') and (Result <> 'standard') and (Result <> 'complete') then
      Result := 'unknown';
  end;
end;
