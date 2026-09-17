<script setup lang="ts">
import { cloudAccessAction } from "@kanna/stream-client"
import { useKannaStore } from "../stores/kanna"
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { openUrl } from '@tauri-apps/plugin-opener'
import {
  AGENT_PROVIDERS,
  isAgentProvider,
} from "@kanna/agent-protocol"
import type { AgentProvider } from "../types/kanna"
import type { AgentExecutionType } from "../stores/agentExecutionType"
import { invoke } from "../invoke"
import {
  useEmbeddableView,
  type EmbeddableViewProps,
} from '../composables/useEmbeddableView'
import { isTopModal } from '../composables/useModalZIndex'
import MobileAccessPanel from './MobileAccessPanel.vue'
import MachinesPanel from './MachinesPanel.vue'
import {
  DESKTOP_PEER_LEGACY_ACCESS_ALLOWED,
  DESKTOP_PEER_LEGACY_ACCESS_REFUSED,
  DESKTOP_PEER_LEGACY_ACCESS_SETTING,
  MOBILE_LEGACY_ACCESS_ALLOWED,
  MOBILE_LEGACY_ACCESS_REFUSED,
  MOBILE_LEGACY_ACCESS_SETTING,
  confirmPendingPairing,
  createDesktopPeerPairingOffer,
  fetchDesktopPeers,
  fetchMobileDevices,
  fetchPendingPairingConfirmation,
  pairDesktopPeer,
  putDesktopSetting,
  rejectPendingPairing,
  removeDesktopPeer,
  type DesktopMobileDevice,
  type DesktopPeer,
  type DesktopPeerPairingOffer,
  type DesktopPendingPairingConfirmation,
} from '../services/desktopServerClient'
import { macOsTextInputAttrs } from '../utils/textInput'
import {
  getConfiguredDesktopAuthSession,
  getConfiguredDesktopPortalBaseUrl,
} from '../services/desktopAuthSdk'
import type { DesktopAuthSession, DesktopAuthState } from '../services/desktopAuth'
import type { MobilePushRegistrationStatus } from '../types/mobilePushRegistration'
import type { AppThemePreference, CodeThemePreference } from '../theme/theme'
import type { AgentMessageAppearance } from '../stores/state'

const store = useKannaStore()
useI18n()
const isDev = import.meta.env.DEV

type MobileServerStatus = "running" | "stopped" | "error"

interface MobileServerStatusResponse {
  desktopId?: string
  desktopName?: string
  state?: string
  pairingCode?: string | null
  environment?: string
}

interface PairingSessionResponse {
  desktopId?: string
  code?: string | null
  pairingPayload?: string | null
  desktopName?: string
  expiresAtUnixMs?: number
}

const props = defineProps<EmbeddableViewProps & {
  preferences: {
    suspendAfterMinutes: number
    killAfterMinutes: number
    ideCommand: string
    terminalEditorCommand?: string
    locale: string
    devLingerTerminals: boolean
    defaultAgentProvider: AgentProvider
    defaultAgentType: AgentExecutionType
    appTheme: AppThemePreference
    codeTheme: CodeThemePreference
    agentMessageAppearance: AgentMessageAppearance
  }
}>()

const emit = defineEmits<{
  update: [key: string, value: string]
  close: []
}>()

const {
  zIndex,
  overlayClass,
  overlayStyle,
  dismissOnScrimClick,
  bringToFront: raiseToFront,
} = useEmbeddableView(props)

type PreferencesTab = 'general' | 'account' | 'mobile' | 'machines' | 'developer'
const activeTab = ref<PreferencesTab>('general')

const tabs: PreferencesTab[] = isDev
  ? ['general', 'account', 'mobile', 'machines', 'developer']
  : ['general', 'account', 'mobile', 'machines']
const mobileDesktopName = ref("This desktop")
const mobileEnvironment = ref("development")
const mobileDesktopId = ref("")
const mobileStatusLoading = ref(false)
const mobileStatusError = ref<string | null>(null)
const pairingPending = ref(false)
const pairingError = ref<string | null>(null)
let statusGeneration = 0
let pushGeneration = 0
const mobileServerStatus = ref<MobileServerStatus>("stopped")
const pairingCode = ref<string | null>(null)
const pairingPayload = ref<string | null>(null)
const pairingExpiresAtUnixMs = ref<number | null>(null)
const pushRegistration = ref<MobilePushRegistrationStatus | null>(null)
const pushRegistrationLoading = ref(false)
const pendingPairingConfirmation = ref<DesktopPendingPairingConfirmation | null>(null)
const pairingConfirmationBusy = ref(false)
const pairingConfirmationError = ref<string | null>(null)
const mobileDevices = ref<DesktopMobileDevice[]>([])
const legacyAccessAllowed = ref(true)
const legacyAccessBusy = ref(false)
const peers = ref<DesktopPeer[]>([])
const peersLoading = ref(false)
const peersError = ref<string | null>(null)
const peerChannelAvailable = ref(true)
const relayPeerTunnelsAvailable = ref(true)
const peerLegacyAccessAllowed = ref(true)
const peerLegacyAccessBusy = ref(false)
const peerOffer = ref<DesktopPeerPairingOffer | null>(null)
const peerOfferPending = ref(false)
const peerOfferError = ref<string | null>(null)
const peerPairPending = ref(false)
const peerPairError = ref<string | null>(null)
const peerPairSuccess = ref<string | null>(null)
const removingPeerDesktopId = ref<string | null>(null)
let peersTimer: ReturnType<typeof setInterval> | null = null
let pairingConfirmationTimer: ReturnType<typeof setInterval> | null = null
const authSession = ref<DesktopAuthSession | null>(null)
const authState = ref<DesktopAuthState>({ status: "signedOut" })
const accountEmail = ref("")
const accountPassword = ref("")
const accountPasswordVisible = ref(false)
const accountMessage = ref("")
let unsubscribeAuth: (() => void) | null = null

const cloudAccess = computed(() => {
  const user = authState.value.status === "signedIn" ? authState.value.user : null
  return user && store.cloudAccount?.userId === user.uid ? store.cloudAccount.entitlement : null
})
const cloudAccountMessage = computed(() => cloudAccessAction(cloudAccess.value)
  ?? (cloudAccess.value?.status === "unknown" ? "Cloud access could not be confirmed. Your local data and LAN access remain available."
    : cloudAccess.value?.active ? "Cloud access active." : null))

const isSigningIn = computed(() => authState.value.status === "signingIn")
const signedInUserEmail = computed(() =>
  authState.value.status === "signedIn"
    ? authState.value.user.email ?? authState.value.user.uid
    : null
)
const defaultAgentSelection = computed(() => {
  return props.preferences.defaultAgentProvider
})
const providerOptions = AGENT_PROVIDERS

function cycleTab(direction: -1 | 1) {
  const idx = tabs.indexOf(activeTab.value)
  activeTab.value = tabs[(idx + direction + tabs.length) % tabs.length]
}

function handleKeydown(e: KeyboardEvent) {
  if (e.key === "Escape") {
    e.preventDefault()
    emit("close")
  }
}

const overlayRef = ref<HTMLDivElement | null>(null)

function bringToFront() {
  raiseToFront()
  void nextTick(() => overlayRef.value?.focus())
}

function isOnTop() {
  return isTopModal(zIndex.value)
}

function normalizeMobileServerStatus(status?: string): MobileServerStatus {
  if (status === "running" || status === "stopped" || status === "error") {
    return status
  }
  return "error"
}

async function refreshMobileAccess() {
  const generation = ++statusGeneration
  mobileStatusLoading.value = true
  mobileStatusError.value = null
  try {
    const status = await invoke<MobileServerStatusResponse>("mobile_server_status")
    if (generation !== statusGeneration) return
    mobileDesktopId.value = status.desktopId?.trim() ?? ""
    mobileEnvironment.value = status.environment?.trim() || "development"
    if (status.desktopName) {
      mobileDesktopName.value = status.desktopName
    }
    mobileServerStatus.value = normalizeMobileServerStatus(status.state)
    // Status has no QR payload or expiry. Only a locally created session owns credentials.
    // The pairing-confirmation poll and the device list need the server the
    // status just proved reachable; they start after it, and only while
    // the Mobile tab is showing.
    if (activeTab.value === "mobile" && mobileServerStatus.value === "running") {
      startPairingConfirmationPolling()
    }
  } catch (error) {
    console.error("[PreferencesPanel] failed to load mobile access status:", error)
    if (generation === statusGeneration) {
      mobileServerStatus.value = "error"
      mobileStatusError.value = error instanceof Error ? error.message : String(error)
    }
  } finally {
    if (generation === statusGeneration) mobileStatusLoading.value = false
  }
}

/**
 * Ask kanna-server whether the signed-in account has a registered push
 * device. The relay decides this through the same target resolution a real
 * `kanna_notify_mobile` would use, without sending anything.
 */
async function refreshPushRegistration() {
  const generation = ++pushGeneration
  if (authState.value.status !== "signedIn") {
    pushRegistration.value = null
    pushRegistrationLoading.value = false
    return
  }
  pushRegistrationLoading.value = true
  try {
    const result = await invoke<MobilePushRegistrationStatus>("mobile_push_registration_status")
    if (generation === pushGeneration) pushRegistration.value = result
  } catch (error) {
    console.error("[PreferencesPanel] failed to load push registration status:", error)
    if (generation === pushGeneration) pushRegistration.value = {
      status: "unavailable",
      registeredDeviceCount: 0,
      error: error instanceof Error ? error.message : String(error)
    }
  } finally {
    if (generation === pushGeneration) pushRegistrationLoading.value = false
  }
}

/**
 * A typed-code pairing parks on the server until the person compares the
 * short authentication strings; the desktop has no push from the server
 * to this panel, so it polls while the Mobile tab is open.
 */
async function refreshPairingConfirmation() {
  try {
    pendingPairingConfirmation.value = await fetchPendingPairingConfirmation()
  } catch (error) {
    console.error("[PreferencesPanel] failed to read pending pairing confirmation:", error)
  }
}

async function refreshMobileDevices() {
  try {
    const summary = await fetchMobileDevices()
    mobileDevices.value = summary.devices
    legacyAccessAllowed.value = summary.legacyAccessAllowed
  } catch (error) {
    console.error("[PreferencesPanel] failed to read paired mobile devices:", error)
  }
}

function startPairingConfirmationPolling() {
  stopPairingConfirmationPolling()
  void refreshPairingConfirmation()
  void refreshMobileDevices()
  pairingConfirmationTimer = setInterval(() => {
    void refreshPairingConfirmation()
  }, 1500)
}

function stopPairingConfirmationPolling() {
  if (pairingConfirmationTimer) clearInterval(pairingConfirmationTimer)
  pairingConfirmationTimer = null
}

// The decision names the request that was on screen when the button was
// pressed; the server refuses it if a later claim replaced that request.
async function confirmPairing(confirmation: DesktopPendingPairingConfirmation) {
  if (pairingConfirmationBusy.value) return
  pairingConfirmationBusy.value = true
  pairingConfirmationError.value = null
  try {
    await confirmPendingPairing(confirmation)
    pendingPairingConfirmation.value = null
    await refreshMobileDevices()
  } catch (error) {
    console.error("[PreferencesPanel] failed to confirm pairing:", error)
    pairingConfirmationError.value = error instanceof Error ? error.message : String(error)
  } finally {
    pairingConfirmationBusy.value = false
  }
}

async function rejectPairing(confirmation: DesktopPendingPairingConfirmation) {
  if (pairingConfirmationBusy.value) return
  pairingConfirmationBusy.value = true
  pairingConfirmationError.value = null
  try {
    await rejectPendingPairing(confirmation)
    pendingPairingConfirmation.value = null
  } catch (error) {
    console.error("[PreferencesPanel] failed to reject pairing:", error)
    pairingConfirmationError.value = error instanceof Error ? error.message : String(error)
  } finally {
    pairingConfirmationBusy.value = false
  }
}

async function setLegacyAccess(allowed: boolean) {
  if (legacyAccessBusy.value) return
  legacyAccessBusy.value = true
  try {
    await putDesktopSetting(
      MOBILE_LEGACY_ACCESS_SETTING,
      allowed ? MOBILE_LEGACY_ACCESS_ALLOWED : MOBILE_LEGACY_ACCESS_REFUSED,
    )
    legacyAccessAllowed.value = allowed
  } catch (error) {
    console.error("[PreferencesPanel] failed to update legacy mobile access:", error)
  } finally {
    legacyAccessBusy.value = false
  }
}

const isSignedIn = computed(() => authState.value.status === "signedIn")
watch(
  () => authState.value.status === "signedIn" ? authState.value.user.uid : null,
  () => {
    ++pushGeneration
    pushRegistration.value = null
    pushRegistrationLoading.value = false
    if (activeTab.value === "mobile") void refreshPushRegistration()
  },
  { flush: "sync" }
)
watch(activeTab, (tab) => {
  if (tab === "mobile") {
    void refreshMobileAccess()
    void refreshPushRegistration()
  } else {
    stopPairingConfirmationPolling()
  }
  if (tab === "machines") {
    void refreshMobileAccess()
    startPeersPolling()
  } else {
    stopPeersPolling()
  }
})

async function refreshPeers() {
  peersLoading.value = true
  try {
    const list = await fetchDesktopPeers()
    peers.value = list.peers
    peerChannelAvailable.value = list.peerChannelAvailable
    relayPeerTunnelsAvailable.value = list.relayPeerTunnelsAvailable
    peerLegacyAccessAllowed.value = list.legacyAccessAllowed
    if (list.desktopName) mobileDesktopName.value = list.desktopName
    if (list.desktopId) mobileDesktopId.value = list.desktopId
    peersError.value = null
  } catch (error) {
    console.error("[PreferencesPanel] failed to list paired machines:", error)
    peersError.value = error instanceof Error ? error.message : String(error)
  } finally {
    peersLoading.value = false
  }
}

// A sibling that pastes this desktop's string pairs on the server; the
// panel polls while it is open so the new machine appears without a click.
function startPeersPolling() {
  stopPeersPolling()
  void refreshPeers()
  peersTimer = setInterval(() => { void refreshPeers() }, 3000)
}

function stopPeersPolling() {
  if (peersTimer) clearInterval(peersTimer)
  peersTimer = null
}

async function createPeerOffer() {
  if (peerOfferPending.value) return
  peerOfferPending.value = true
  peerOfferError.value = null
  try {
    peerOffer.value = await createDesktopPeerPairingOffer()
  } catch (error) {
    console.error("[PreferencesPanel] failed to create a peer pairing string:", error)
    peerOfferError.value = error instanceof Error ? error.message : String(error)
  } finally {
    peerOfferPending.value = false
  }
}

async function pairWithString(pairingString: string) {
  if (peerPairPending.value) return
  peerPairPending.value = true
  peerPairError.value = null
  peerPairSuccess.value = null
  try {
    const result = await pairDesktopPeer(pairingString)
    peerPairSuccess.value = `Paired with ${result.displayName} (${result.route}).`
    await refreshPeers()
  } catch (error) {
    console.error("[PreferencesPanel] peer pairing failed:", error)
    peerPairError.value = error instanceof Error ? error.message : String(error)
  } finally {
    peerPairPending.value = false
  }
}

async function removePeer(desktopId: string) {
  if (removingPeerDesktopId.value) return
  removingPeerDesktopId.value = desktopId
  try {
    await removeDesktopPeer(desktopId)
    await refreshPeers()
  } catch (error) {
    console.error("[PreferencesPanel] failed to unpair machine:", error)
    peersError.value = error instanceof Error ? error.message : String(error)
  } finally {
    removingPeerDesktopId.value = null
  }
}

async function setPeerLegacyAccess(allowed: boolean) {
  if (peerLegacyAccessBusy.value) return
  peerLegacyAccessBusy.value = true
  try {
    await putDesktopSetting(
      DESKTOP_PEER_LEGACY_ACCESS_SETTING,
      allowed ? DESKTOP_PEER_LEGACY_ACCESS_ALLOWED : DESKTOP_PEER_LEGACY_ACCESS_REFUSED,
    )
    peerLegacyAccessAllowed.value = allowed
  } catch (error) {
    console.error("[PreferencesPanel] failed to update legacy desktop-to-desktop access:", error)
  } finally {
    peerLegacyAccessBusy.value = false
  }
}

async function openAccountSettings() {
  activeTab.value = "account"
  await nextTick()
  overlayRef.value?.querySelector<HTMLButtonElement>('[data-testid="preferences-account-tab"]')?.focus()
}

async function startPairing() {
  if (pairingPending.value) return
  pairingPending.value = true
  pairingError.value = null
  // A new request may replace the server session even if its response is lost.
  pairingCode.value = null
  pairingPayload.value = null
  pairingExpiresAtUnixMs.value = null
  try {
    const session = await invoke<PairingSessionResponse>("create_mobile_pairing_session")
    mobileDesktopId.value = session.desktopId?.trim() ?? mobileDesktopId.value
    if (session.desktopName) {
      mobileDesktopName.value = session.desktopName
    }
    ++statusGeneration
    mobileStatusLoading.value = false
    mobileStatusError.value = null
    mobileServerStatus.value = "running"
    pairingCode.value = session.code ?? null
    pairingPayload.value = session.pairingPayload ?? null
    pairingExpiresAtUnixMs.value = session.expiresAtUnixMs ?? null
  } catch (error) {
    console.error("[PreferencesPanel] failed to create pairing session:", error)
    pairingError.value = error instanceof Error ? error.message : String(error)
  } finally {
    pairingPending.value = false
  }
}

async function refreshAccountSession() {
  try {
    const session = await getConfiguredDesktopAuthSession()
    authSession.value = session
    await session.initialize()
    unsubscribeAuth?.()
    unsubscribeAuth = session.subscribe((state) => {
      authState.value = state
      if (state.status === "signedIn") {
        accountMessage.value = ""
        accountPassword.value = ""
      } else if (state.status === "error") {
        accountMessage.value = state.message
      }
    })
  } catch (error) {
    accountMessage.value = error instanceof Error ? error.message : "Failed to initialize sign-in."
  }
}

async function signInAccount() {
  accountMessage.value = ""
  await authSession.value?.signInWithEmailPassword({
    email: accountEmail.value,
    password: accountPassword.value,
  })
}

async function signOutAccount() {
  accountMessage.value = ""
  const result = await authSession.value?.signOut()
  // Sign-out is the only thing that releases this desktop for another account.
  // If it did not land, say so now — the next account is refused by the cloud
  // rules, and this is the last moment the previous one can still release it.
  if (result?.desktopCredentialError) {
    accountMessage.value =
      `Signed out, but this desktop was not released from the previous account `
      + `(${result.desktopCredentialError}). Sign back in as that account and sign out `
      + `again, or the next account cannot use cloud sync on this machine.`
  }
}

async function openAccountPortal(path: "/register" | "/account") {
  accountMessage.value = ""
  try {
    const baseUrl = await getConfiguredDesktopPortalBaseUrl()
    await openUrl(`${baseUrl}${path}`)
  } catch (error) {
    console.error("[PreferencesPanel] failed to open the account portal:", error)
    accountMessage.value = "Could not open the Kanna account portal."
  }
}

function handleDefaultAgentChange(value: string) {
  if (!isAgentProvider(value)) return
  emit("update", "defaultAgentProvider", value)
  emit("update", "defaultAgentType", "pty")
}

onMounted(() => {
  overlayRef.value?.focus()
  void refreshMobileAccess()
  void refreshAccountSession()
})

onBeforeUnmount(() => {
  unsubscribeAuth?.()
  stopPairingConfirmationPolling()
  stopPeersPolling()
  ++pushGeneration
  ++statusGeneration
})

defineExpose({ bringToFront, cycleTab, isOnTop })
</script>

<template>
  <div
    ref="overlayRef"
    :class="overlayClass"
    :style="overlayStyle"
    tabindex="-1"
    role="dialog"
    aria-modal="true"
    :aria-label="$t('preferences.title')"
    @click.self="dismissOnScrimClick(() => emit('close'))"
    @keydown="handleKeydown"
  >
    <div class="prefs-panel">
      <div class="prefs-header">
        <div class="tab-bar">
          <button
            class="tab"
            :class="{ active: activeTab === 'general' }"
            @click="activeTab = 'general'"
          >{{ $t('preferences.title') }}</button>
          <button
            class="tab"
            data-testid="preferences-account-tab"
            :class="{ active: activeTab === 'account' }"
            @click="activeTab = 'account'"
          >Account</button>
          <button
            class="tab"
            data-testid="preferences-mobile-tab"
            :class="{ active: activeTab === 'mobile' }"
            @click="activeTab = 'mobile'"
          >Mobile</button>
          <button
            class="tab"
            data-testid="preferences-machines-tab"
            :class="{ active: activeTab === 'machines' }"
            @click="activeTab = 'machines'"
          >Machines</button>
          <button
            v-if="isDev"
            class="tab"
            :class="{ active: activeTab === 'developer' }"
            @click="activeTab = 'developer'"
          >Developer</button>
        </div>
      </div>

      <div v-if="activeTab === 'general'" class="prefs-body">
        <div class="pref-row">
          <label>{{ $t('preferences.language') }}</label>
          <select
            :value="preferences.locale"
            @change="emit('update', 'locale', ($event.target as HTMLSelectElement).value)"
          >
            <option value="en">English</option>
            <option value="ja">日本語</option>
            <option value="ko">한국어</option>
          </select>
        </div>

        <div class="pref-row">
          <label>{{ $t('preferences.theme') }}</label>
          <select
            data-testid="app-theme-select"
            :value="preferences.appTheme"
            @change="emit('update', 'appTheme', ($event.target as HTMLSelectElement).value)"
          >
            <option value="system">{{ $t('preferences.themeSystem') }}</option>
            <option value="light">{{ $t('preferences.themeLight') }}</option>
            <option value="dark">{{ $t('preferences.themeDark') }}</option>
          </select>
        </div>

        <div class="pref-row">
          <label>{{ $t('preferences.codeTheme') }}</label>
          <select
            data-testid="code-theme-select"
            :value="preferences.codeTheme"
            @change="emit('update', 'codeTheme', ($event.target as HTMLSelectElement).value)"
          >
            <option value="match">{{ $t('preferences.codeThemeMatch') }}</option>
            <option value="light">{{ $t('preferences.codeThemeLight') }}</option>
            <option value="dark">{{ $t('preferences.codeThemeDark') }}</option>
          </select>
        </div>

        <div class="pref-row">
          <label>{{ $t('preferences.agentMessageAppearance') }}</label>
          <select
            data-testid="agent-message-appearance-select"
            :value="preferences.agentMessageAppearance"
            @change="emit('update', 'agentMessageAppearance', ($event.target as HTMLSelectElement).value)"
          >
            <option value="chat">{{ $t('preferences.agentMessageAppearanceChat') }}</option>
            <option value="log">{{ $t('preferences.agentMessageAppearanceLog') }}</option>
            <option value="terminal">{{ $t('preferences.agentMessageAppearanceTerminal') }}</option>
          </select>
        </div>

        <div class="pref-row">
          <label>{{ $t('preferences.suspendAfter') }}</label>
          <input
            type="number"
            :value="preferences.suspendAfterMinutes"
            min="1"
            @change="emit('update', 'suspendAfterMinutes', ($event.target as HTMLInputElement).value)"
          />
        </div>

        <div class="pref-row">
          <label>{{ $t('preferences.killAfter') }}</label>
          <input
            type="number"
            :value="preferences.killAfterMinutes"
            min="5"
            @change="emit('update', 'killAfterMinutes', ($event.target as HTMLInputElement).value)"
          />
        </div>

        <div class="pref-row">
          <label>{{ $t('preferences.ideCommand') }}</label>
          <input
            type="text"
            v-bind="macOsTextInputAttrs"
            :value="preferences.ideCommand"
            :placeholder="$t('preferences.idePlaceholder')"
            @change="emit('update', 'ideCommand', ($event.target as HTMLInputElement).value)"
          />
        </div>

        <div class="pref-row">
          <label for="terminal-editor-command">Terminal Editor Command</label>
          <input id="terminal-editor-command" type="text" v-bind="macOsTextInputAttrs"
            :value="preferences.terminalEditorCommand ?? ''" placeholder="Auto-detect (nvim, vim, nano…)"
            @change="emit('update', 'terminalEditorCommand', ($event.target as HTMLInputElement).value)" />
        </div>
        <p class="pref-hint">Optional installed terminal editor, e.g. nvim or emacs -nw. Quotes and arguments are supported; shell expressions are not. Leave empty to choose from detected editors. Graphical editors belong in IDE Command.</p>

        <div class="pref-row">
          <label>{{ $t('preferences.defaultAgent') }}</label>
          <select
            data-testid="default-agent-select"
            :value="defaultAgentSelection"
            @change="handleDefaultAgentChange(($event.target as HTMLSelectElement).value)"
          >
            <option v-for="provider in providerOptions" :key="provider" :value="provider">
              {{ provider }}
            </option>
          </select>
        </div>
      </div>

      <div v-if="activeTab === 'account'" class="prefs-body">
        <section class="account-panel">
          <div v-if="mobileDesktopId" class="desktop-identity">
            <span class="account-label">Desktop ID</span>
            <code>{{ mobileDesktopId }}</code>
          </div>

          <div v-if="signedInUserEmail" class="account-signed-in">
            <p v-if="cloudAccountMessage" class="account-help" role="status" data-testid="account-cloud-access">{{ cloudAccountMessage }}</p>
            <span class="account-label">Signed in</span>
            <strong>{{ signedInUserEmail }}</strong>
            <button
              type="button"
              class="secondary-button"
              data-testid="account-manage-subscription"
              @click="openAccountPortal('/account')"
            >
              Manage subscription
            </button>
            <p class="account-help">Opens the web portal. Sign in with your Kanna account.</p>
            <button
              type="button"
              class="secondary-button"
              data-testid="account-sign-out"
              @click="signOutAccount"
            >
              Sign out
            </button>
          </div>

          <form v-else class="account-form" data-testid="account-sign-in" @submit.prevent="signInAccount">
            <label class="account-field">
              <span>Email</span>
              <input
                v-model="accountEmail"
                data-testid="account-email"
                v-bind="macOsTextInputAttrs"
                type="email"
                autocomplete="email"
                required
              />
            </label>

            <label class="account-field">
              <span>Password</span>
              <span class="password-input-row">
                <input
                  v-model="accountPassword"
                  data-testid="account-password"
                  v-bind="macOsTextInputAttrs"
                  :type="accountPasswordVisible ? 'text' : 'password'"
                  autocomplete="current-password"
                  required
                />
                <button
                  type="button"
                  class="password-toggle"
                  data-testid="account-toggle-password"
                  :aria-label="accountPasswordVisible ? 'Hide password' : 'Show password'"
                  @click="accountPasswordVisible = !accountPasswordVisible"
                >
                  {{ accountPasswordVisible ? "Hide" : "Show" }}
                </button>
              </span>
            </label>

            <button type="submit" class="primary-button" :disabled="isSigningIn">
              {{ isSigningIn ? "Signing in..." : "Sign in" }}
            </button>
            <button
              type="button"
              class="account-link"
              data-testid="account-create"
              @click="openAccountPortal('/register')"
            >
              Create account
            </button>
          </form>

          <p v-if="accountMessage" class="account-message">{{ accountMessage }}</p>
        </section>
      </div>

      <div v-if="activeTab === 'mobile'" class="prefs-body mobile-body">
        <MobileAccessPanel
          :desktop-name="mobileDesktopName"
          :environment="mobileEnvironment"
          :server-status="mobileServerStatus"
          :status-loading="mobileStatusLoading"
          :status-error="mobileStatusError"
          :pairing-pending="pairingPending"
          :pairing-error="pairingError"
          :pairing-code="pairingCode"
          :pairing-payload="pairingPayload"
          :expires-at-unix-ms="pairingExpiresAtUnixMs"
          :account-signed-in="isSignedIn"
          :push-registration="pushRegistration"
          :push-registration-loading="pushRegistrationLoading"
          :pending-confirmation="pendingPairingConfirmation"
          :confirmation-busy="pairingConfirmationBusy"
          :confirmation-error="pairingConfirmationError"
          :devices="mobileDevices"
          :legacy-access-allowed="legacyAccessAllowed"
          :legacy-access-busy="legacyAccessBusy"
          @start-pairing="startPairing"
          @refresh-status="refreshMobileAccess"
          @open-account="openAccountSettings"
          @refresh-push-registration="refreshPushRegistration"
          @confirm-pairing="confirmPairing"
          @reject-pairing="rejectPairing"
          @set-legacy-access="setLegacyAccess"
        />
      </div>

      <div v-if="activeTab === 'machines'" class="prefs-body mobile-body">
        <MachinesPanel
          :desktop-name="mobileDesktopName"
          :desktop-id="mobileDesktopId"
          :peers="peers"
          :peers-loading="peersLoading"
          :peers-error="peersError"
          :peer-channel-available="peerChannelAvailable"
          :relay-peer-tunnels-available="relayPeerTunnelsAvailable"
          :offer="peerOffer"
          :offer-pending="peerOfferPending"
          :offer-error="peerOfferError"
          :pair-pending="peerPairPending"
          :pair-error="peerPairError"
          :pair-success="peerPairSuccess"
          :legacy-access-allowed="peerLegacyAccessAllowed"
          :legacy-access-busy="peerLegacyAccessBusy"
          :removing-desktop-id="removingPeerDesktopId"
          @create-offer="createPeerOffer"
          @pair="pairWithString"
          @remove-peer="removePeer"
          @refresh="refreshPeers"
          @set-legacy-access="setPeerLegacyAccess"
        />
      </div>

      <div v-if="activeTab === 'developer'" class="prefs-body">
        <div class="pref-row">
          <label>Linger terminals after teardown</label>
          <input
            type="checkbox"
            :checked="preferences.devLingerTerminals"
            @change="emit('update', 'dev.lingerTerminals', ($event.target as HTMLInputElement).checked ? 'true' : 'false')"
          />
        </div>
      </div>

      <div class="prefs-footer">
        <button class="btn-done" @click="emit('close')">{{ $t('actions.done') }}</button>
      </div>
    </div>
  </div>
</template>

<style scoped>
.embedded .prefs-panel {
  width: 100%;
  height: 100%;
  max-width: none;
  border: none;
  border-radius: 0;
}

.modal-overlay {
  position: fixed;
  inset: 0;
  background: var(--kn-overlay-scrim);
  display: flex;
  align-items: center;
  justify-content: center;
  outline: none;
}

.prefs-panel {
  background: var(--kn-bg-panel);
  border: 1px solid var(--kn-border-strong);
  border-radius: 8px;
  width: 420px;
  max-width: 90vw;
  min-height: 280px;
  display: flex;
  flex-direction: column;
  box-shadow: var(--kn-shadow-modal);
}


.prefs-header {
  border-bottom: 1px solid var(--kn-border-default);
}

.tab-bar {
  display: flex;
  padding: 0 12px;
}

.tab {
  padding: 10px 12px 8px;
  font-size: 13px;
  font-weight: 500;
  color: var(--kn-text-muted);
  background: none;
  border: none;
  border-bottom: 2px solid transparent;
  cursor: pointer;
  transition: color 0.15s, border-color 0.15s;
}

.tab:hover {
  color: var(--kn-text-secondary);
}

.tab.active {
  color: var(--kn-text-primary);
  border-bottom-color: var(--kn-accent);
}

.prefs-body {
  flex: 1;
  padding: 12px 16px;
  display: flex;
  flex-direction: column;
  gap: 10px;
}

.mobile-body {
  max-height: calc(90vh - 110px);
  overflow-y: auto;
}

.pref-hint {
  margin: 0;
  font-size: 12px;
  line-height: 1.5;
  color: var(--kn-text-muted);
}

.pref-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
}

.pref-row label {
  font-size: 13px;
  color: var(--kn-text-secondary);
  flex: 1;
  white-space: nowrap;
}

.pref-row input[type="number"],
.pref-row input[type="text"],
.account-field input {
  background: var(--kn-bg-input);
  border: 1px solid var(--kn-border-strong);
  border-radius: 4px;
  color: var(--kn-text-primary);
  font-size: 12px;
  padding: 5px 8px;
  width: 160px;
  outline: none;
  font-family: -apple-system, BlinkMacSystemFont, "SF Pro Text", sans-serif;
}

.pref-row input[type="number"] {
  width: 80px;
}

.pref-row input:focus {
  border-color: var(--kn-accent);
}

.pref-row input[type="checkbox"] {
  accent-color: var(--kn-accent);
  width: 14px;
  height: 14px;
  cursor: pointer;
}

.account-panel {
  display: flex;
  flex-direction: column;
  gap: 12px;
}

.account-form {
  display: flex;
  flex-direction: column;
  gap: 10px;
}

.account-field {
  display: flex;
  flex-direction: column;
  gap: 5px;
  font-size: 12px;
  color: var(--kn-text-secondary);
}

.account-field input {
  width: 100%;
  box-sizing: border-box;
}

.password-input-row {
  display: flex;
  gap: 6px;
}

.password-input-row input {
  flex: 1;
  min-width: 0;
}

.password-toggle {
  border: 1px solid var(--kn-border-strong);
  border-radius: 4px;
  background: var(--kn-bg-hover);
  color: var(--kn-text-secondary);
  cursor: pointer;
  font-family: -apple-system, BlinkMacSystemFont, "SF Pro Text", sans-serif;
  font-size: 12px;
  font-weight: 600;
  padding: 5px 9px;
}

.password-toggle:hover {
  color: var(--kn-text-primary);
}

.account-signed-in {
  display: flex;
  flex-direction: column;
  gap: 8px;
  color: var(--kn-text-primary);
}

.desktop-identity {
  display: flex;
  flex-direction: column;
  gap: 5px;
}

.desktop-identity code {
  align-self: flex-start;
  max-width: 100%;
  box-sizing: border-box;
  padding: 5px 8px;
  border: 1px solid var(--kn-border-strong);
  border-radius: 5px;
  background: var(--kn-bg-input);
  color: var(--kn-text-primary);
  font-size: 12px;
  line-height: 1.3;
  overflow-wrap: anywhere;
}

.account-label {
  font-size: 11px;
  color: var(--kn-text-muted);
  text-transform: uppercase;
  letter-spacing: 0.06em;
}

.account-message {
  margin: 0;
  color: var(--kn-danger);
  font-size: 12px;
  line-height: 1.4;
}

.account-help {
  margin: -2px 0 0;
  color: var(--kn-text-muted);
  font-size: 12px;
  line-height: 1.4;
}

.account-link {
  align-self: flex-start;
  padding: 0;
  border: 0;
  background: none;
  color: var(--kn-accent);
  cursor: pointer;
  font: inherit;
  font-size: 12px;
  text-decoration: underline;
}

.primary-button,
.secondary-button {
  align-self: flex-start;
  padding: 7px 12px;
  border-radius: 5px;
  font-size: 12px;
  font-weight: 600;
  cursor: pointer;
}

.primary-button {
  border: 1px solid var(--kn-accent-hover);
  background: var(--kn-accent);
  color: var(--kn-text-inverse);
}

.primary-button:disabled {
  opacity: 0.65;
  cursor: default;
}

.secondary-button {
  border: 1px solid var(--kn-border-strong);
  background: var(--kn-bg-hover);
  color: var(--kn-text-secondary);
}

.pref-row select {
  background: var(--kn-bg-input);
  border: 1px solid var(--kn-border-strong);
  border-radius: 4px;
  color: var(--kn-text-primary);
  font-size: 12px;
  padding: 5px 8px;
  width: 160px;
  outline: none;
  font-family: -apple-system, BlinkMacSystemFont, "SF Pro Text", sans-serif;
}

.pref-row select:focus {
  border-color: var(--kn-accent);
}

.prefs-footer {
  display: flex;
  justify-content: flex-end;
  padding: 10px 16px 14px;
  border-top: 1px solid var(--kn-border-default);
}

.btn-done {
  padding: 5px 20px;
  background: var(--kn-accent);
  border: 1px solid var(--kn-accent-hover);
  border-radius: 4px;
  color: var(--kn-text-inverse);
  font-size: 12px;
  font-weight: 500;
  cursor: pointer;
}

.btn-done:hover {
  background: var(--kn-accent-hover);
}
</style>
