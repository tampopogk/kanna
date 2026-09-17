import { getAppErrorMessage } from "../appError"
import { invoke } from "../invoke"
import {
  buildKittyClipboardResponse,
  collectKittyClipboardRequests,
  type ClipboardImagePayload,
  encodeTerminalPasteBytes,
  formatDroppedPathsForPaste,
  updateBracketedPasteMode,
} from "./terminalMediaBridge"
import type { TerminalOptions } from "./terminalTypes"

const CLIPBOARD_IMAGE_TTL_MS = 30_000
const BRACKETED_PASTE_CONTROL_SEQUENCE = /\u001b\[\?2004[hl]/

export interface TerminalClipboardBridge {
  maybeReadClipboardImage(): Promise<void>
  readClipboardText(): Promise<string | null>
  handleTerminalOutputControlSequences(bytes: Uint8Array): void
  restoreTerminalModesFromSnapshot(serializedTerminalState: string): void
  sendDroppedPaths(paths: string[]): void
  reset(): void
}

export function createTerminalClipboardBridge(params: {
  sessionId: string
  instanceId: string
  options?: TerminalOptions
  outputDecoder: TextDecoder
  sendInputBytes: (
    bytes: Uint8Array,
    config?: { immediate?: boolean; submissionBoundary?: boolean; controlInput?: boolean },
  ) => Promise<void>
}): TerminalClipboardBridge {
  let bracketedPasteMode = false
  let hasObservedBracketedPasteMode = false
  let kittyClipboardBuffer = ""
  let pendingClipboardImage: ClipboardImagePayload | null = null
  let pendingClipboardImageExpiresAt = 0
  let pendingClipboardImageLoad: Promise<ClipboardImagePayload | null> | null = null

  function clearPendingClipboardImage() {
    pendingClipboardImage = null
    pendingClipboardImageExpiresAt = 0
  }

  function getPendingClipboardImage(): ClipboardImagePayload | null {
    if (!pendingClipboardImage) {
      return null
    }
    if (Date.now() > pendingClipboardImageExpiresAt) {
      clearPendingClipboardImage()
      return null
    }
    return pendingClipboardImage
  }

  function armPendingClipboardImage(payload: ClipboardImagePayload) {
    pendingClipboardImage = payload
    pendingClipboardImageExpiresAt = Date.now() + CLIPBOARD_IMAGE_TTL_MS
  }

  async function maybeReadClipboardImage() {
    if (!params.options?.agentTerminal) {
      return
    }
    const load = (async () => {
      try {
        const payload = await invoke<ClipboardImagePayload | null>("read_clipboard_image_png", {})
        if (!payload) {
          clearPendingClipboardImage()
          return null
        }
        armPendingClipboardImage(payload)
        return payload
      } catch (error) {
        clearPendingClipboardImage()
        console.warn("[terminal][clipboard] failed to read clipboard image", {
          sessionId: params.sessionId,
          instanceId: params.instanceId,
          error: getAppErrorMessage(error),
        })
        return null
      }
    })()
    pendingClipboardImageLoad = load
    void load.finally(() => {
      if (pendingClipboardImageLoad === load) {
        pendingClipboardImageLoad = null
      }
    })
  }

  /**
   * The clipboard's text, read natively.
   *
   * Not `navigator.clipboard.readText()`: WebKitGTK denies that by policy, so
   * the terminal's Linux paste chord could never resolve it. The native read
   * is the only one that works there, and it is the same `arboard` backend
   * the image read above already goes through.
   *
   * A denial or a clipboard that holds no text is `null` — the caller pastes
   * nothing — and the reason is logged rather than swallowed.
   */
  async function readClipboardText(): Promise<string | null> {
    try {
      const text = await invoke<string | null>("read_clipboard_text", {})
      return text ? text : null
    } catch (error) {
      console.warn("[terminal][clipboard] failed to read clipboard text", {
        sessionId: params.sessionId,
        instanceId: params.instanceId,
        error: getAppErrorMessage(error),
      })
      return null
    }
  }

  async function resolvePendingClipboardImage(): Promise<ClipboardImagePayload | null> {
    const readyPayload = getPendingClipboardImage()
    if (readyPayload) {
      return readyPayload
    }
    if (!pendingClipboardImageLoad) {
      return null
    }
    await pendingClipboardImageLoad
    return getPendingClipboardImage()
  }

  async function maybeRespondToKittyClipboardRequests(
    requests: ReturnType<typeof collectKittyClipboardRequests>["requests"],
  ) {
    if (requests.length === 0) {
      return
    }

    const payload = await resolvePendingClipboardImage()
    if (!payload) {
      return
    }

    const matchesRequest = requests.some((request) => {
      return request.mimeTypes.length === 0 || request.mimeTypes.includes(payload.mimeType)
    })

    if (!matchesRequest) {
      return
    }

    clearPendingClipboardImage()
    const response = buildKittyClipboardResponse(payload)
    await params.sendInputBytes(new TextEncoder().encode(response), {
      immediate: true,
      controlInput: true,
    })
  }

  function handleTerminalOutputControlSequences(bytes: Uint8Array) {
    const chunkText = params.outputDecoder.decode(bytes, { stream: true })
    if (BRACKETED_PASTE_CONTROL_SEQUENCE.test(chunkText)) {
      hasObservedBracketedPasteMode = true
    }
    bracketedPasteMode = updateBracketedPasteMode(bracketedPasteMode, chunkText)
    kittyClipboardBuffer += chunkText

    const parsed = collectKittyClipboardRequests(kittyClipboardBuffer)
    kittyClipboardBuffer = parsed.remainder
    void maybeRespondToKittyClipboardRequests(parsed.requests).catch((error) => {
      console.warn("[terminal][clipboard] failed to send clipboard image response", {
        sessionId: params.sessionId,
        instanceId: params.instanceId,
        error: getAppErrorMessage(error),
      })
    })
  }

  function restoreTerminalModesFromSnapshot(serializedTerminalState: string) {
    // AttachSnapshot/recovery snapshots redraw the terminal from a serialized VT stream.
    // Replay the mode toggles embedded in that stream so local paste behavior
    // matches the restored terminal state.
    if (BRACKETED_PASTE_CONTROL_SEQUENCE.test(serializedTerminalState)) {
      hasObservedBracketedPasteMode = true
    }
    bracketedPasteMode = updateBracketedPasteMode(false, serializedTerminalState)
  }

  function shouldUseBracketedPasteForDrop() {
    if (params.options?.agentProvider === "copilot" && !hasObservedBracketedPasteMode) {
      return true
    }
    return bracketedPasteMode
  }

  function sendDroppedPaths(paths: string[]) {
    if (paths.length === 0) return
    const text = formatDroppedPathsForPaste(paths)
    const bytes = encodeTerminalPasteBytes(text, shouldUseBracketedPasteForDrop())
    void params.sendInputBytes(bytes, { immediate: true })
  }

  function reset() {
    clearPendingClipboardImage()
    pendingClipboardImageLoad = null
    kittyClipboardBuffer = ""
    bracketedPasteMode = false
    hasObservedBracketedPasteMode = false
  }

  return {
    maybeReadClipboardImage,
    readClipboardText,
    handleTerminalOutputControlSequences,
    restoreTerminalModesFromSnapshot,
    sendDroppedPaths,
    reset,
  }
}
