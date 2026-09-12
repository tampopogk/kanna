<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { renderPairingQr, renderQrCode } from "../utils/pairingQr";
import { getMobileInstallLink } from "../utils/mobileInstallLinks";
import type { MobilePushRegistrationStatus } from "../types/mobilePushRegistration";

const { t, locale } = useI18n();
const props = defineProps<{
  desktopName: string;
  environment?: string;
  serverStatus: "running" | "stopped" | "error";
  statusLoading?: boolean;
  statusError?: string | null;
  pairingPending?: boolean;
  pairingError?: string | null;
  pairingCode: string | null;
  pairingPayload: string | null;
  expiresAtUnixMs?: number | null;
  accountSignedIn?: boolean;
  pushRegistration?: MobilePushRegistrationStatus | null;
  pushRegistrationLoading?: boolean;
}>();
const emit = defineEmits<{
  (e: "start-pairing"): void;
  (e: "refresh-status"): void;
  (e: "refresh-push-registration"): void;
  (e: "open-account"): void;
}>();
const disclosure = ref<"install" | "pairing" | null>(props.pairingCode ? "pairing" : null);
const troubleshooting = ref(false);
function toggleDisclosure(value: "install" | "pairing") {
  disclosure.value = disclosure.value === value ? null : value;
}
watch(() => props.pairingCode, (code) => { if (code) disclosure.value = "pairing"; });
watch(() => props.pairingPending, (pending) => { if (pending) disclosure.value = "pairing"; });

const installLink = computed(() => getMobileInstallLink(props.environment ?? "development"));
const installQrUrl = ref<string | null>(null);
const installQrError = ref(false);
let installQrGeneration = 0;
async function renderInstallQr() {
  const generation = ++installQrGeneration;
  const link = installLink.value;
  installQrUrl.value = null;
  installQrError.value = false;
  if (!link) return;
  try {
    const url = await renderQrCode(link);
    if (generation === installQrGeneration) installQrUrl.value = url;
  } catch (error) {
    console.error("[MobileAccessPanel] installation QR failed:", error);
    if (generation === installQrGeneration) installQrError.value = true;
  }
}
watch(installLink, renderInstallQr, { immediate: true });
const statusLabel = computed(() => t(`mobileAccess.${props.statusLoading ? "checking" : props.serverStatus === "running" ? "ready" : "unavailable"}`));
const pairingQrUrl = ref<string | null>(null);
const pairingQrError = ref<string | null>(null);
const pairingExpired = ref(false);
const expiryLabel = computed(() => props.expiresAtUnixMs
  ? t("mobileAccess.expiresAt", { time: new Date(props.expiresAtUnixMs).toLocaleTimeString(locale.value, { hour: "numeric", minute: "2-digit", second: "2-digit" }) }) : "");
const pushTone = computed(() => !props.accountSignedIn ? "signedOut"
  : props.pushRegistrationLoading ? "loading" : props.pushRegistration?.status ?? "unavailable");
const pushSummary = computed(() => pushTone.value === "registered"
  ? t("mobileAccess.registered", { count: props.pushRegistration?.registeredDeviceCount ?? 0 })
  : t(`mobileAccess.push_${pushTone.value}`));
let qrGeneration = 0;
let expiryTimer: ReturnType<typeof setTimeout> | null = null;

watch(
  () => [props.pairingCode, props.expiresAtUnixMs] as const,
  ([pairingCode, expiresAtUnixMs]) => {
    if (expiryTimer) clearTimeout(expiryTimer);
    expiryTimer = null;
    pairingExpired.value = Boolean(
      pairingCode && expiresAtUnixMs && expiresAtUnixMs <= Date.now()
    );
    if (!pairingCode || !expiresAtUnixMs || pairingExpired.value) return;

    expiryTimer = setTimeout(() => {
      pairingExpired.value = true;
      expiryTimer = null;
    }, Math.max(0, expiresAtUnixMs - Date.now()));
  },
  { immediate: true },
);

watch(
  () => [props.pairingPayload, pairingExpired.value] as const,
  async ([payload, expired]) => {
    const generation = ++qrGeneration;
    pairingQrUrl.value = null;
    pairingQrError.value = null;
    if (!payload || expired) return;

    try {
      const url = await renderPairingQr(payload);
      if (generation === qrGeneration) pairingQrUrl.value = url;
    } catch (error) {
      console.error("[MobileAccessPanel] pairing QR failed:", error);
      if (generation === qrGeneration) {
        pairingQrError.value = error instanceof Error
          ? error.message
          : "Could not render the pairing QR code.";
      }
    }
  },
  { immediate: true },
);

onBeforeUnmount(() => {
  if (expiryTimer) clearTimeout(expiryTimer);
});
</script>

<template>
  <section class="mobile-access-panel" data-testid="mobile-access-panel">
    <h3>{{ t('mobileAccess.title') }}</h3>
    <p class="desktop-status"><strong>{{ desktopName }}</strong><span data-testid="mobile-access-status" role="status">{{ statusLabel }}</span></p>
    <p v-if="statusError" class="error" role="alert">{{ t('mobileAccess.statusFailed') }}</p>

    <section class="setup-step" data-testid="mobile-access-install">
      <button
        type="button"
        class="disclosure"
        data-testid="mobile-access-install-toggle"
        :aria-expanded="disclosure === 'install'"
        aria-controls="mobile-install-content"
        @click="toggleDisclosure('install')"
      >
        <span>{{ t('mobileAccess.installTitle') }}</span><span aria-hidden="true">{{ disclosure === 'install' ? '−' : '+' }}</span>
      </button>
      <div v-if="disclosure === 'install'" id="mobile-install-content" class="qr-content">
        <p data-testid="mobile-access-install-label">{{ t('mobileAccess.installDestination') }}</p>
        <template v-if="installLink">
          <p>{{ t('mobileAccess.installHint') }}</p>
          <img
            v-if="installQrUrl"
            :src="installQrUrl"
            :alt="t('mobileAccess.installQrAlt')"
            class="qr-image"
            data-testid="mobile-access-install-qr"
          />
          <template v-else-if="installQrError">
            <p class="error" role="alert" data-testid="mobile-access-install-error">{{ t('mobileAccess.installQrError') }}</p>
            <button
              type="button"
              class="secondary-action"
              data-testid="mobile-access-install-retry"
              @click="renderInstallQr"
            >{{ t('mobileAccess.retryRendering') }}</button>
          </template>
          <p v-else role="status">{{ t('mobileAccess.checking') }}</p>
        </template>
        <p v-else data-testid="mobile-access-install-unconfigured">{{ t('mobileAccess.installUnconfigured') }}</p>
      </div>
    </section>

    <section class="setup-step">
      <h4>{{ t('mobileAccess.connectTitle') }}</h4>
      <p>{{ t('mobileAccess.connectHint') }}</p>
      <p>{{ t('mobileAccess.localHint') }}</p>
      <button
        v-if="pairingCode && !pairingExpired"
        type="button"
        class="disclosure"
        data-testid="mobile-access-pairing-toggle"
        :aria-expanded="disclosure === 'pairing'"
        aria-controls="mobile-pairing-content"
        @click="toggleDisclosure('pairing')"
      >{{ t('mobileAccess.pairingQrLabel') }}<span aria-hidden="true">{{ disclosure === 'pairing' ? '−' : '+' }}</span></button>
      <div
        v-if="disclosure === 'pairing' && pairingCode && !pairingExpired"
        id="mobile-pairing-content"
        class="qr-content"
      >
        <p data-testid="mobile-access-pairing-qr-label">{{ t('mobileAccess.pairingHint') }}</p>
        <img
          v-if="pairingQrUrl"
          :src="pairingQrUrl"
          :alt="t('mobileAccess.pairingQrAlt')"
          class="qr-image"
          data-testid="mobile-access-pairing-qr"
        />
        <p v-else-if="pairingQrError" class="error" role="alert">{{ t('mobileAccess.pairingQrError') }}</p>
        <p>{{ t('mobileAccess.manualCode') }} <code data-testid="mobile-access-pairing-code">{{ pairingCode }}</code></p>
        <p v-if="expiryLabel">{{ expiryLabel }}</p>
        <p>{{ t('mobileAccess.pairingSuccessHint') }}</p>
      </div>
      <p v-if="pairingExpired" role="status">{{ t('mobileAccess.expired') }}</p>
      <p v-if="pairingError" class="error" role="alert">{{ t('mobileAccess.pairingFailed') }}</p>
      <button
        type="button"
        class="primary-action"
        data-testid="mobile-access-start-pairing"
        :disabled="pairingPending"
        @click="emit('start-pairing')"
      >{{ t(pairingPending ? 'mobileAccess.creating' : pairingCode ? 'mobileAccess.generateCode' : 'mobileAccess.pairDevice') }}</button>
    </section>

    <section
      class="notifications"
      data-testid="mobile-access-push-registration"
      :data-status="pushTone"
    >
      <h4>{{ t('mobileAccess.notifications') }}</h4>
      <p role="status">{{ pushSummary }}</p>
      <p v-if="pushTone === 'noRegisteredDevices'" data-testid="mobile-access-push-instruction">{{ t('mobileAccess.pushInstruction') }}</p>
      <button v-if="!accountSignedIn" type="button" class="text-action" @click="emit('open-account')">{{ t('mobileAccess.openAccount') }}</button>
    </section>
    <section class="troubleshooting">
      <button
        type="button"
        class="disclosure"
        data-testid="mobile-access-troubleshooting-toggle"
        :aria-expanded="troubleshooting"
        aria-controls="mobile-troubleshooting"
        @click="troubleshooting = !troubleshooting"
      >{{ t('mobileAccess.troubleshooting') }}<span aria-hidden="true">{{ troubleshooting ? '−' : '+' }}</span></button>
      <div v-if="troubleshooting" id="mobile-troubleshooting">
        <p>{{ t('mobileAccess.desktopStatus') }}: {{ statusLabel }}</p>
        <p v-if="statusError" class="error">{{ statusError }}</p>
        <p v-if="pairingError" class="error">{{ pairingError }}</p>
        <button
          type="button"
          class="secondary-action"
          data-testid="mobile-access-status-refresh"
          :disabled="statusLoading"
          @click="emit('refresh-status')"
        >{{ t('mobileAccess.checkAgain') }}</button>
        <template v-if="accountSignedIn">
          <p>{{ t('mobileAccess.probeScope') }}</p>
          <p v-if="pushRegistration?.error" class="error">{{ pushRegistration.error }}</p>
          <div v-if="pushRegistration?.noDevicesReason" data-testid="mobile-access-push-reason">
            <p>{{ pushRegistration.noDevicesReason.message }}</p>
            <p class="diagnostic">{{ [pushRegistration.noDevicesReason.code, pushRegistration.noDevicesReason.retiredAt, pushRegistration.noDevicesReason.providerCode, pushRegistration.noDevicesReason.retiredByDesktopId].filter(Boolean).join(' · ') }}</p>
          </div>
          <button
            type="button"
            class="secondary-action"
            data-testid="mobile-access-push-refresh"
            :disabled="pushRegistrationLoading"
            @click="emit('refresh-push-registration')"
          >{{ t(pushRegistrationLoading ? 'mobileAccess.checking' : 'mobileAccess.checkNotifications') }}</button>
        </template>
      </div>
    </section>
  </section>
</template>

<style scoped>
.mobile-access-panel {
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
.desktop-status {
  display: flex;
  flex-wrap: wrap;
  justify-content: space-between;
  gap: 4px 12px;
}
.desktop-status strong {
  color: var(--kn-text-primary);
}
.setup-step, .notifications, .troubleshooting {
  padding: 14px 0;
  border-top: 1px solid var(--kn-border-default);
}
.setup-step:first-of-type {
  margin-top: 16px;
}
.disclosure {
  display: flex;
  justify-content: space-between;
  gap: 10px;
  width: 100%;
  text-align: left;
  border: 0;
  background: none;
  padding: 4px 0;
  color: var(--kn-text-primary);
  font-size: 13px;
  font-weight: 600;
  cursor: pointer;
}
.qr-content {
  padding-top: 4px;
}
.qr-image {
  display: block;
  width: 220px;
  max-width: 100%;
  height: auto;
  margin: 12px auto;
  border-radius: 8px;
}
code {
  display: inline-block;
  padding: 4px 8px;
  border: 1px solid var(--kn-border-strong);
  border-radius: 5px;
  color: var(--kn-text-primary);
  background: var(--kn-bg-input);
  font-size: 14px;
  letter-spacing: .08em;
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
}
</style>
