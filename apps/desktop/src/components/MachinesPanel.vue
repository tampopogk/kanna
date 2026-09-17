<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { renderQrCode } from "../utils/pairingQr";
import type {
  DesktopPeer,
  DesktopPeerPairingOffer,
} from "../services/desktopServerClient";

const { t, locale } = useI18n();
// Absent optional boolean props are cast to `false` by Vue, so the
// "available"/"allowed" flags default to `true` explicitly: a caller that
// does not know is not a caller reporting an outage.
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
  legacyAccessAllowed?: boolean;
  legacyAccessBusy?: boolean;
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
  legacyAccessAllowed: true,
  legacyAccessBusy: false,
  removingDesktopId: null,
});
const emit = defineEmits<{
  (e: "create-offer"): void;
  (e: "pair", pairingString: string): void;
  (e: "remove-peer", desktopId: string): void;
  (e: "refresh"): void;
  (e: "set-legacy-access", allowed: boolean): void;
}>();

const pairingInput = ref("");
const copied = ref(false);
let copiedTimer: ReturnType<typeof setTimeout> | null = null;
const offerExpired = ref(false);
let expiryTimer: ReturnType<typeof setTimeout> | null = null;
const offerQrUrl = ref<string | null>(null);
let qrGeneration = 0;

const expiryLabel = computed(() => props.offer
  ? t("machines.offerExpiresAt", {
    time: new Date(props.offer.expiresAtUnixMs).toLocaleTimeString(locale.value, { hour: "numeric", minute: "2-digit", second: "2-digit" }),
  })
  : "");

watch(
  () => props.offer?.expiresAtUnixMs ?? null,
  (expiresAtUnixMs) => {
    if (expiryTimer) clearTimeout(expiryTimer);
    expiryTimer = null;
    offerExpired.value = Boolean(expiresAtUnixMs && expiresAtUnixMs <= Date.now());
    if (!expiresAtUnixMs || offerExpired.value) return;
    expiryTimer = setTimeout(() => {
      offerExpired.value = true;
      expiryTimer = null;
    }, Math.max(0, expiresAtUnixMs - Date.now()));
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
  if (expiryTimer) clearTimeout(expiryTimer);
  if (copiedTimer) clearTimeout(copiedTimer);
});
</script>

<template>
  <section class="machines-panel" data-testid="machines-panel">
    <h3>{{ t('machines.title') }}</h3>
    <p class="intro">{{ t('machines.intro') }}</p>
    <p v-if="!peerChannelAvailable" class="error" role="alert" data-testid="machines-channel-unavailable">
      {{ t('machines.channelUnavailable') }}
    </p>

    <section class="step">
      <h4>{{ t('machines.offerTitle') }}</h4>
      <p>{{ t('machines.offerHint') }}</p>
      <button
        type="button"
        class="primary-action"
        data-testid="machines-create-offer"
        :disabled="offerPending || !peerChannelAvailable"
        @click="emit('create-offer')"
      >{{ t(offerPending ? 'machines.creatingOffer' : offer ? 'machines.newOffer' : 'machines.createOffer') }}</button>
      <p v-if="offerError" class="error" role="alert">{{ offerError }}</p>
      <div v-if="offer && !offerExpired" class="offer" data-testid="machines-offer">
        <code data-testid="machines-offer-string">{{ offer.pairingString }}</code>
        <div class="offer-actions">
          <button type="button" class="secondary-action" data-testid="machines-copy-offer" @click="copyOffer">
            {{ t(copied ? 'machines.copied' : 'machines.copy') }}
          </button>
          <span v-if="expiryLabel" class="diagnostic">{{ expiryLabel }}</span>
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
    </section>

    <section class="step">
      <h4>{{ t('machines.pairTitle') }}</h4>
      <p>{{ t('machines.pairHint') }}</p>
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

    <section class="step peers" data-testid="machines-peers">
      <h4>{{ t('machines.peersTitle') }}</h4>
      <p v-if="peersError" class="error" role="alert">{{ peersError }}</p>
      <p v-else-if="peersLoading && peers.length === 0" role="status">{{ t('machines.loading') }}</p>
      <p v-else-if="peers.length === 0" data-testid="machines-no-peers">{{ t('machines.noPeers') }}</p>
      <ul v-else class="peer-list">
        <li
          v-for="peer in peers"
          :key="peer.desktopId"
          :data-testid="`machines-peer-${peer.desktopId}`"
        >
          <div class="peer-main">
            <span class="peer-name">{{ peer.displayName }}</span>
            <span class="badge badge-secure">{{ t('machines.peerEncrypted') }}</span>
          </div>
          <div class="peer-meta diagnostic">
            <span>{{ peer.desktopId }}</span>
            <span>{{ reachability(peer) }}</span>
            <span v-if="!peer.transferIdentityPinned">{{ t('machines.transferPending') }}</span>
          </div>
          <button
            type="button"
            class="text-action"
            :data-testid="`machines-remove-${peer.desktopId}`"
            :disabled="removingDesktopId === peer.desktopId"
            @click="emit('remove-peer', peer.desktopId)"
          >{{ t('machines.unpair') }}</button>
        </li>
      </ul>
      <button type="button" class="secondary-action" data-testid="machines-refresh" @click="emit('refresh')">
        {{ t('machines.refresh') }}
      </button>
    </section>

    <section class="step">
      <label class="legacy-toggle">
        <input
          type="checkbox"
          data-testid="machines-legacy-toggle"
          :checked="legacyAccessAllowed"
          :disabled="legacyAccessBusy"
          @change="emit('set-legacy-access', ($event.target as HTMLInputElement).checked)"
        />
        <span>{{ t('machines.legacyToggle') }}</span>
      </label>
      <p class="diagnostic">{{ t('machines.legacyHint') }}</p>
      <p v-if="!relayPeerTunnelsAvailable" class="diagnostic" data-testid="machines-relay-note">
        {{ t('machines.relayUnavailable') }}
      </p>
    </section>
  </section>
</template>

<style scoped>
.machines-panel {
  color: var(--kn-text-primary);
  font-size: 12px;
  line-height: 1.5;
}
h3 {
  margin: 0 0 8px;
  font-size: 16px;
}
h4 {
  margin: 0 0 8px;
  font-size: 13px;
}
p {
  margin: 8px 0;
  color: var(--kn-text-secondary);
  overflow-wrap: anywhere;
}
.step {
  padding: 14px 0;
  border-top: 1px solid var(--kn-border-default);
}
.step:first-of-type {
  margin-top: 16px;
}
.offer code {
  display: block;
  padding: 6px 8px;
  border: 1px solid var(--kn-border-strong);
  border-radius: 5px;
  color: var(--kn-text-primary);
  background: var(--kn-bg-input);
  font-size: 11px;
  overflow-wrap: anywhere;
  user-select: all;
}
.offer-actions {
  display: flex;
  align-items: center;
  gap: 10px;
  margin: 8px 0;
}
.qr-image {
  display: block;
  width: 220px;
  max-width: 100%;
  height: auto;
  margin: 12px auto;
  border-radius: 8px;
}
.pair-form {
  display: flex;
  gap: 8px;
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
.peer-list {
  list-style: none;
  margin: 0 0 8px;
  padding: 0;
}
.peer-list li {
  display: grid;
  grid-template-columns: 1fr auto;
  gap: 2px 12px;
  padding: 6px 0;
  border-bottom: 1px solid var(--kn-border-default);
}
.peer-main {
  display: flex;
  align-items: center;
  gap: 8px;
}
.peer-name {
  font-weight: 600;
}
.peer-meta {
  grid-column: 1;
  display: flex;
  flex-wrap: wrap;
  gap: 4px 12px;
  color: var(--kn-text-secondary);
}
.peer-list li .text-action {
  grid-row: 1 / span 2;
  grid-column: 2;
  align-self: center;
}
.badge {
  border-radius: 999px;
  padding: 0 8px;
  font-size: 11px;
  font-weight: 600;
}
.badge-secure {
  background: rgba(60, 170, 110, 0.18);
  color: #6fd39c;
}
.legacy-toggle {
  display: flex;
  align-items: center;
  gap: 6px;
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
  margin-top: 4px;
}
.secondary-action {
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
.diagnostic {
  font-family: monospace;
  font-size: 11px;
}
</style>
