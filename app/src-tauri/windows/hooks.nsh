; Extra steps for the NSIS installer (wired up in tauri.conf.json).
; SwiftDL registers itself with browsers on every launch (per user, under
; HKCU). Remove those entries on uninstall so nothing is left behind.

!define SWIFTDL_HOST "com.swiftdl.host"
!define SWIFTDL_EXT_ID "knmfjbmojepkdiflnckpfjimfmjfcbgh"

!macro SWIFTDL_UNREGISTER ROOT
  DeleteRegKey HKCU "${ROOT}\NativeMessagingHosts\${SWIFTDL_HOST}"
  DeleteRegKey HKCU "${ROOT}\Extensions\${SWIFTDL_EXT_ID}"
!macroend

; Magnet links and .torrent files: remove SwiftDL's registration, and the
; magnet / .torrent defaults only while they still point to SwiftDL.
!macro SWIFTDL_UNREGISTER_HANDLERS
  DeleteRegKey HKCU "Software\Classes\SwiftDL.Torrent"
  DeleteRegKey HKCU "Software\SwiftDL"
  DeleteRegValue HKCU "Software\RegisteredApplications" "SwiftDL"
  DeleteRegValue HKCU "Software\Classes\.torrent\OpenWithProgids" "SwiftDL.Torrent"
  ReadRegStr $0 HKCU "Software\Classes\.torrent" ""
  StrCmp $0 "SwiftDL.Torrent" 0 +2
    DeleteRegValue HKCU "Software\Classes\.torrent" ""
  ReadRegStr $0 HKCU "Software\Classes\magnet\shell\open\command" ""
  Push $0
  Push "swiftdl"
  Call un.SwiftDLContains
  Pop $1
  StrCmp $1 "1" 0 +2
    DeleteRegKey HKCU "Software\Classes\magnet"
!macroend

; Leaves 1 on the stack if the first string contains the second (any case).
Function un.SwiftDLContains
  Exch $R1 ; needle
  Exch
  Exch $R0 ; haystack
  Push $R2
  Push $R3
  Push $R4
  StrLen $R3 $R1
  StrCpy $R4 0
  loop:
    StrCpy $R2 $R0 $R3 $R4
    StrCmp $R2 "" notfound
    StrCmp $R2 $R1 found
    IntOp $R4 $R4 + 1
    Goto loop
  found:
    StrCpy $R0 1
    Goto done
  notfound:
    StrCpy $R0 0
  done:
  Pop $R4
  Pop $R3
  Pop $R2
  Pop $R1
  Exch $R0
FunctionEnd

!macro NSIS_HOOK_POSTUNINSTALL
  !insertmacro SWIFTDL_UNREGISTER_HANDLERS
  !insertmacro SWIFTDL_UNREGISTER "Software\Google\Chrome"
  !insertmacro SWIFTDL_UNREGISTER "Software\Microsoft\Edge"
  !insertmacro SWIFTDL_UNREGISTER "Software\BraveSoftware\Brave-Browser"
  !insertmacro SWIFTDL_UNREGISTER "Software\Vivaldi"
  !insertmacro SWIFTDL_UNREGISTER "Software\Chromium"
!macroend
