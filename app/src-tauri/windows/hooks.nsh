; Extra steps for the NSIS installer (wired up in tauri.conf.json).
; SwiftDL registers itself with browsers on every launch (per user, under
; HKCU). Remove those entries on uninstall so nothing is left behind.

!define SWIFTDL_HOST "com.swiftdl.host"
!define SWIFTDL_EXT_ID "knmfjbmojepkdiflnckpfjimfmjfcbgh"

!macro SWIFTDL_UNREGISTER ROOT
  DeleteRegKey HKCU "${ROOT}\NativeMessagingHosts\${SWIFTDL_HOST}"
  DeleteRegKey HKCU "${ROOT}\Extensions\${SWIFTDL_EXT_ID}"
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  !insertmacro SWIFTDL_UNREGISTER "Software\Google\Chrome"
  !insertmacro SWIFTDL_UNREGISTER "Software\Microsoft\Edge"
  !insertmacro SWIFTDL_UNREGISTER "Software\BraveSoftware\Brave-Browser"
  !insertmacro SWIFTDL_UNREGISTER "Software\Vivaldi"
  !insertmacro SWIFTDL_UNREGISTER "Software\Chromium"
!macroend
