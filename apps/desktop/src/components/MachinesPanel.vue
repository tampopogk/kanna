<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { renderQrCode } from "../utils/pairingQr";
import type {
  DesktopPeer,
  DesktopPeerPairingOffer,
} from "../services/desktopServerClient";

const { t } = useI18n();
// Absent optional boolean props are cast to `false` by Vue, so the
// "available" flags default to `true` explicitly: a caller that does not
// know is not a caller reporting an outage.
const props = withDefaults(defineProps<{
  desktopName: string;
  desktopId: string;
  peers: DesktopPeer[];
  peersLoading?: boolean;
  peersError?: string | null;
  peerChannelAvailable?: boolean;
  relayPeerTunnelsAvailable?: boolean;
  /** The one-time string on this desktop's screen, if one is live. */
  offer?: DesktopPeerPairingOffer | null;
  offerPending?: boolean;
  offerError?: string | null;
  pairPending?: boolean;
  pairError?: string | null;
  pairSuccess?: string | null;
  removingDesktopId?: string | null;
}>(), {
  peersLoading: false,
  peersError: null,
  peerChannelAvailable: true,
  relayPeerTunnelsAvailable: true,
  offer: null,
  offerPending: false,
  offerError: null,
  pairPending: false,
  pairError: null,
  pairSuccess: null,
  removingDesktopId: null,
});
const emit = defineEmits<{
  (e: "create-offer"): void;
  (e: "pair", pairingString: string): void;
  (e: "remove-peer", desktopId: string): void;
}>();

const pairingInput = ref("");
const copied = ref(false);
let copiedTimer: ReturnType<typeof setTimeout> | null = null;
const offerExpired = ref(false);
let expiryTimer: ReturnType<typeof setTimeout> | null = null;
/** Ticks once a second while an offer is live, so the countdown is live. */
const now = ref(Date.now());
let countdownTimer: ReturnType<typeof setInterval> | null = null;
const offerQrUrl = ref<string | null>(null);
let qrGeneration = 0;

const countdownLabel = computed(() => {
  const expiresAtUnixMs = props.offer?.expiresAtUnixMs;
  if (!expiresAtUnixMs || offerExpired.value) return "";
  const remainingSeconds = Math.max(0, Math.ceil((expiresAtUnixMs - now.value) / 1000));
  const minutes = Math.floor(remainingSeconds / 60);
  const seconds = String(remainingSeconds % 60).padStart(2, "0");
  return t("machines.offerExpiresIn", { countdown: `${minutes}:${seconds}` });
});

watch(
  () => props.offer?.expiresAtUnixMs ?? null,
  (expiresAtUnixMs) => {
    clearOfferTimers();
    now.value = Date.now();
    offerExpired.value = Boolean(expiresAtUnixMs && expiresAtUnixMs <= now.value);
    if (!expiresAtUnixMs || offerExpired.value) return;
    // The timeout keeps expiry exact; the interval only drives the display.
    expiryTimer = setTimeout(() => {
      offerExpired.value = true;
      clearOfferTimers();
    }, Math.max(0, expiresAtUnixMs - now.value));
    countdownTimer = setInterval(() => { now.value = Date.now(); }, 1000);
  },
  { immediate: true },
);

watch(
  () => [props.offer?.pairingString ?? null, offerExpired.value] as const,
  async ([pairingString, expired]) => {
    const generation = ++qrGeneration;
    offerQrUrl.value = null;
    if (!pairingString || expired) return;
    try {
      const url = await renderQrCode(pairingString);
      if (generation === qrGeneration) offerQrUrl.value = url;
    } catch (error) {
      console.error("[MachinesPanel] pairing QR failed:", error);
    }
  },
  { immediate: true },
);

function clearOfferTimers() {
  if (expiryTimer) clearTimeout(expiryTimer);
  expiryTimer = null;
  if (countdownTimer) clearInterval(countdownTimer);
  countdownTimer = null;
}

async function copyOffer() {
  const pairingString = props.offer?.pairingString;
  if (!pairingString) return;
  try {
    await navigator.clipboard.writeText(pairingString);
    copied.value = true;
    if (copiedTimer) clearTimeout(copiedTimer);
    copiedTimer = setTimeout(() => { copied.value = false; }, 2000);
  } catch (error) {
    console.error("[MachinesPanel] clipboard write failed:", error);
  }
}

function submitPairing() {
  const value = pairingInput.value.trim();
  if (!value || props.pairPending) return;
  emit("pair", value);
}

watch(() => props.pairSuccess, (success) => { if (success) pairingInput.value = ""; });

function reachability(peer: DesktopPeer): string {
  if (peer.reachable.lan) return t("machines.reachableLan");
  if (peer.reachable.relay) return t("machines.reachableRelay");
  return t("machines.reachableNone");
}

onBeforeUnmount(() => {
  clearOfferTimers();
  if (copiedTimer) clearTimeout(copiedTimer);
});
</script>

<template>
  <section class="machines-panel" data-testid="machines-panel">
    <p v-if="!peerChannelAvailable" class="error" role="alert" data-testid="machines-channel-unavailable">
      {{ t('machines.channelUnavailable') }}
    </p>

    <section class="block" data-testid="machines-peers">
      <h4>{{ t('machines.peersTitle') }}</h4>
      <p v-if="peersError" class="error" role="alert">{{ peersError }}</p>
      <p v-else-if="peersLoading && peers.length === 0" role="status">{{ t('machines.loading') }}</p>
      <p v-else-if="peers.length === 0" data-testid="machines-no-peers">{{ t('machines.noPeers') }}</p>
      <ul v-else class="peer-list">
        <li
          v-for="peer in peers"
          :key="peer.desktopId"
          :title="peer.desktopId"
          :data-testid="`machines-peer-${peer.desktopId}`"
        >
          <span class="peer-name">{{ peer.displayName }}</span>
          <span
            class="badge"
            :class="peer.provenance === 'account' ? 'badge-account' : 'badge-verified'"
            :data-testid="`machines-peer-provenance-${peer.desktopId}`"
          >{{ t(peer.provenance === 'account' ? 'machines.peerAccount' : 'machines.peerVerified') }}</span>
          <button
            type="button"
            class="text-action"
            :data-testid="`machines-remove-${peer.desktopId}`"
            :disabled="removingDesktopId === peer.desktopId"
            @click="emit('remove-peer', peer.desktopId)"
          >{{ t('machines.unpair') }}</button>
          <p class="peer-meta">
            <span>{{ reachability(peer) }}</span>
            <span v-if="!peer.transferIdentityPinned">{{ t('machines.transferPending') }}</span>
          </p>
          <p
            v-if="peer.identityChanged"
            class="peer-alert error"
            role="alert"
            :data-testid="`machines-peer-identity-changed-${peer.desktopId}`"
          >{{ t('machines.peerIdentityChanged') }}</p>
        </li>
      </ul>
    </section>

    <section class="block">
      <h4>{{ t('machines.pairTitle') }}</h4>
      <!-- Show-mine first: it is the step that produces something, and its
           output belongs directly under the button that made it. Paste-theirs
           sits last, next to its own button. -->
      <button
        v-if="!offer || offerExpired"
        type="button"
        class="secondary-action"
        data-testid="machines-create-offer"
        :disabled="offerPending || !peerChannelAvailable"
        @click="emit('create-offer')"
      >{{ t(offerPending ? 'machines.creatingOffer' : offer ? 'machines.newOffer' : 'machines.createOffer') }}</button>
      <p v-if="offerError" class="error" role="alert">{{ offerError }}</p>
      <div v-if="offer && !offerExpired" class="offer" data-testid="machines-offer">
        <div class="offer-code">
          <button
            type="button"
            class="offer-string"
            data-testid="machines-offer-string"
            :title="t('machines.copyHint')"
            :aria-label="t('machines.copyHint')"
            @click="copyOffer"
          >{{ offer.pairingString }}</button>
          <p class="muted offer-meta">
            <span data-testid="machines-offer-meta">{{ copied ? t('machines.copied') : countdownLabel }}</span>
            <button
              type="button"
              class="icon-action"
              data-testid="machines-new-offer"
              :disabled="offerPending"
              :title="t('machines.newOffer')"
              :aria-label="t('machines.newOffer')"
              @click="emit('create-offer')"
            >
              <svg width="14" height="14" viewBox="0 0 16 16" aria-hidden="true">
                <path d="M14 8a6 6 0 1 1-1.8-4.2" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" />
                <path d="M13.8 1.2v3.2h-3.2" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" />
              </svg>
            </button>
          </p>
        </div>
        <img
          v-if="offerQrUrl"
          :src="offerQrUrl"
          :alt="t('machines.offerQrAlt')"
          class="qr-image"
          data-testid="machines-offer-qr"
        />
      </div>
      <p v-else-if="offer && offerExpired" role="status" data-testid="machines-offer-expired">{{ t('machines.offerExpired') }}</p>

      <form class="pair-form" @submit.prevent="submitPairing">
        <input
          v-model="pairingInput"
          type="text"
          autocomplete="off"
          spellcheck="false"
          data-testid="machines-pairing-input"
          :placeholder="t('machines.pairPlaceholder')"
          :disabled="pairPending"
        />
        <button
          type="submit"
          class="primary-action"
          data-testid="machines-pair"
          :disabled="pairPending || !pairingInput.trim()"
        >{{ t(pairPending ? 'machines.pairing' : 'machines.pair') }}</button>
      </form>
      <p v-if="pairError" class="error" role="alert" data-testid="machines-pair-error">{{ pairError }}</p>
      <p v-if="pairSuccess" role="status" data-testid="machines-pair-success">{{ pairSuccess }}</p>
    </section>

    <p v-if="!relayPeerTunnelsAvailable" class="muted" data-testid="machines-relay-note">
      {{ t('machines.relayUnavailable') }}
    </p>
  </section>
</template>

<style scoped>
.machines-panel {
  color: var(--kn-text-primary);
  font-size: 12px;
  line-height: 1.5;
}
h4 {
  margin: 0;
  font-size: 13px;
}
p {
  margin: 8px 0;
  color: var(--kn-text-secondary);
  overflow-wrap: anywhere;
}
.block {
  margin-top: 16px;
  padding-top: 14px;
  border-top: 1px solid var(--kn-border-default);
}
.block:first-child {
  margin-top: 0;
  padding-top: 0;
  border-top: 0;
}
.peer-list {
  list-style: none;
  margin: 8px 0 0;
  padding: 0;
}
.peer-list li {
  display: grid;
  grid-template-columns: auto auto 1fr auto;
  align-items: baseline;
  gap: 2px 8px;
  padding: 6px 0;
  border-top: 1px solid var(--kn-border-default);
}
.peer-list li:first-child {
  border-top: 0;
}
.peer-name {
  font-weight: 600;
}
.peer-list li .text-action {
  grid-column: 4;
}
.peer-meta, .peer-alert {
  grid-column: 1 / -1;
  display: flex;
  flex-wrap: wrap;
  gap: 2px 8px;
  margin: 0;
  font-size: 11px;
}
.peer-meta span + span::before {
  content: "·";
  margin-right: 8px;
}
.badge {
  border-radius: 999px;
  padding: 0 8px;
  font-size: 11px;
  font-weight: 600;
}
.badge-verified {
  color: var(--kn-success);
  background: var(--kn-success-bg);
}
/* Neutral on purpose: an automatically pinned sibling is real end-to-end
   encryption, but it is not the claim a key carried between two screens
   makes, so it does not get the affirmative colour. */
.badge-account {
  color: var(--kn-text-secondary);
  background: var(--kn-bg-input);
  box-shadow: inset 0 0 0 1px var(--kn-border-default);
}
.pair-form {
  display: flex;
  gap: 8px;
  margin-top: 10px;
}
.pair-form input {
  flex: 1;
  min-width: 0;
  padding: 6px 8px;
  border: 1px solid var(--kn-border-strong);
  border-radius: 5px;
  background: var(--kn-bg-input);
  color: var(--kn-text-primary);
  font: inherit;
}
.offer-string {
  display: block;
  width: 100%;
  padding: 6px 8px;
  border: 1px solid var(--kn-border-strong);
  border-radius: 5px;
  color: var(--kn-text-primary);
  background: var(--kn-bg-input);
  font-family: monospace;
  font-size: 11px;
  text-align: left;
  overflow-wrap: anywhere;
  cursor: pointer;
}
.offer-string:hover {
  border-color: var(--kn-accent);
}
.offer {
  display: flex;
  align-items: center;
  gap: 12px;
  margin-top: 8px;
}
.offer-code {
  flex: 1;
  min-width: 0;
}
.offer-meta {
  display: flex;
  align-items: center;
  gap: 8px;
  margin: 6px 0 0;
}
.icon-action {
  display: inline-flex;
  padding: 0;
  border: 0;
  background: none;
  color: var(--kn-text-muted);
  cursor: pointer;
}
.icon-action:hover {
  color: var(--kn-accent);
}
.qr-image {
  display: block;
  flex: none;
  width: 120px;
  height: auto;
  border-radius: 6px;
}
.primary-action, .secondary-action {
  padding: 8px 12px;
  border-radius: 6px;
  font-size: 12px;
  cursor: pointer;
}
.primary-action {
  border: 1px solid var(--kn-accent);
  background: var(--kn-accent);
  color: var(--kn-text-inverse);
}
.secondary-action {
  margin-top: 10px;
  border: 1px solid var(--kn-border-strong);
  background: var(--kn-bg-input);
  color: var(--kn-text-primary);
}
.text-action {
  border: 0;
  padding: 0;
  background: none;
  color: var(--kn-accent);
  text-decoration: underline;
  cursor: pointer;
  font: inherit;
  font-size: 12px;
}
button:disabled {
  opacity: .6;
  cursor: default;
}
button:focus-visible {
  outline: 2px solid var(--kn-accent);
  outline-offset: 3px;
}
.error {
  color: var(--kn-danger);
}
.muted {
  color: var(--kn-text-secondary);
  font-size: 11px;
}
</style>
