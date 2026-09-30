; Registers PdfShrinker as an *additional* "Open with" choice for PDFs, without
; touching the default handler. Tauri's own `fileAssociations` would overwrite
; the default value of `Software\Classes\.pdf` (see its FileAssociation.nsh),
; so it's emptied in tauri.windows.conf.json and these hooks are used instead.

!define PDFSHRINK_PROGID "PdfShrinker.pdf"

!macro NSIS_HOOK_POSTINSTALL
  WriteRegStr SHCTX "Software\Classes\${PDFSHRINK_PROGID}" "" "PDF Document"
  WriteRegStr SHCTX "Software\Classes\${PDFSHRINK_PROGID}\DefaultIcon" "" "$\"$INSTDIR\${MAINBINARYNAME}.exe$\",0"
  WriteRegStr SHCTX "Software\Classes\${PDFSHRINK_PROGID}\shell\open\command" "" "$\"$INSTDIR\${MAINBINARYNAME}.exe$\" $\"%1$\""
  ; Listed in .pdf's "Open with" menu; the (Default) value is left alone.
  WriteRegStr SHCTX "Software\Classes\.pdf\OpenWithProgids" "${PDFSHRINK_PROGID}" ""

  WriteRegStr SHCTX "Software\Classes\Applications\${MAINBINARYNAME}.exe" "FriendlyAppName" "${PRODUCTNAME}"
  WriteRegStr SHCTX "Software\Classes\Applications\${MAINBINARYNAME}.exe\SupportedTypes" ".pdf" ""
  WriteRegStr SHCTX "Software\Classes\Applications\${MAINBINARYNAME}.exe\shell\open\command" "" "$\"$INSTDIR\${MAINBINARYNAME}.exe$\" $\"%1$\""

  ; SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_FLUSH): refresh Explorer's menus.
  System::Call "shell32::SHChangeNotify(i 0x08000000, i 0x1000, i 0, i 0)"
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  DeleteRegValue SHCTX "Software\Classes\.pdf\OpenWithProgids" "${PDFSHRINK_PROGID}"
  DeleteRegKey SHCTX "Software\Classes\${PDFSHRINK_PROGID}"
  DeleteRegKey SHCTX "Software\Classes\Applications\${MAINBINARYNAME}.exe"
  System::Call "shell32::SHChangeNotify(i 0x08000000, i 0x1000, i 0, i 0)"
!macroend
