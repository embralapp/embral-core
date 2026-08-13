; Hooked into the generated installer.nsi via `bundle.windows.nsis.installerHooks`.
;
; Tauri includes this file before it inserts MUI_PAGE_WELCOME, and MUI only
; falls back to its stock welcome text when MUI_WELCOMEPAGE_TEXT is undefined
; - so defining it here replaces that text. The stock line tells people to
; close all other applications before continuing, which is advice a per-user
; install with no shared files does not need.
;
; Keep this file ASCII: the script is built with `Unicode true`, and NSIS
; reads an included file as ANSI unless it carries a UTF-8 BOM.
;
; The macro bodies below use LogicLib; that is safe even though this file
; is included early, because macro bodies expand where they are inserted
; (deep inside the template's sections), not where they are defined.

!define MUI_WELCOMEPAGE_TEXT "Meeting transcription and notes, on your machine.$\r$\n$\r$\n$_CLICK"

; --- The MCP server vs. updates ([release.md] Installer hooks) ---
;
; embral-mcp.exe is spawned by MCP clients (and by the app itself, as the
; embedding worker) and can be running while this installer writes files.
; Windows will not let a running image be deleted or overwritten, but it
; WILL let it be renamed - the process keeps serving from the renamed
; file. So: a locked server binary is renamed aside as
; embral-mcp.exe.stale-N and a copy is put back under the real name, so
; the path MCP clients have registered never points at nothing even if
; the install aborts after this hook; the template's File write then
; replaces that (unlocked) copy with the new build. Leftover .stale files
; are swept here on the next install, by the app at boot, and by the
; uninstaller - each pass skipping whatever is still locked.
;
; If every rename attempt fails (an unlocked file never enters this path,
; so that means something exotic like an AV scan), the hook falls through
; and the installer behaves exactly as it did before this file existed.

; Best-effort removal of renamed-aside server binaries. Inserted in more
; than one hook, so: LogicLib only, no raw labels.
!macro EMBRAL_SWEEP_STALE_SERVERS
  Push $R7
  Push $R8
  ClearErrors
  FindFirst $R7 $R8 "$INSTDIR\embral-mcp.exe.stale-*"
  ${IfNot} ${Errors}
    ${Do}
      Delete "$INSTDIR\$R8"
      ClearErrors
      FindNext $R7 $R8
      ${If} ${Errors}
        ${ExitDo}
      ${EndIf}
    ${Loop}
    FindClose $R7
  ${EndIf}
  ClearErrors
  Pop $R8
  Pop $R7
!macroend

!macro NSIS_HOOK_PREINSTALL
  !insertmacro EMBRAL_SWEEP_STALE_SERVERS
  ${If} ${FileExists} "$INSTDIR\embral-mcp.exe"
    Push $R7
    Push $R8
    ; Probe: openable for append means not locked, and the plain File
    ; overwrite later in the install works - nothing to do.
    ClearErrors
    FileOpen $R7 "$INSTDIR\embral-mcp.exe" a
    ${If} ${Errors}
      StrCpy $R8 0
      ${Do}
        ClearErrors
        Rename "$INSTDIR\embral-mcp.exe" "$INSTDIR\embral-mcp.exe.stale-$R8"
        ${IfNot} ${Errors}
          CopyFiles /SILENT "$INSTDIR\embral-mcp.exe.stale-$R8" "$INSTDIR\embral-mcp.exe"
          ${ExitDo}
        ${EndIf}
        IntOp $R8 $R8 + 1
      ${LoopUntil} $R8 > 9
      ClearErrors
    ${Else}
      FileClose $R7
    ${EndIf}
    Pop $R8
    Pop $R7
  ${EndIf}
!macroend

; PRE, not POST: the uninstaller's RMDir $INSTDIR runs before the post
; hook, so a post-hook sweep would leave an empty directory behind.
!macro NSIS_HOOK_PREUNINSTALL
  !insertmacro EMBRAL_SWEEP_STALE_SERVERS
!macroend
