<script setup lang="ts">
import { computed, onScopeDispose, ref, watch } from "vue";
import { RouterLink } from "vue-router";
import { usePortalFirebase, usePortalSession } from "../session";

import CloudAccessStatus from "../components/CloudAccessStatus.vue";
import SetupHandoff from "../components/SetupHandoff.vue";

const props = withDefaults(defineProps<{ redirect?: (url: string) => void }>(), {
  redirect: (url: string) => window.location.assign(url),
});
const session = usePortalSession();
const api = usePortalFirebase();
const deleting = ref(false);
const confirmation = ref("");
const pending = ref(false);
const error = ref("");

const billingPending = ref(false);
const billingError = ref("");
const source = computed(() => session.entitlement.value?.source);
const hasStripe = computed(() => source.value === "stripe" || Boolean(session.entitlement.value?.stripeCustomerId));
let generation = 0;
watch(() => session.user.value?.uid, () => {
  ++generation;
  billingPending.value = false;
  billingError.value = "";
  deleting.value = false;
  confirmation.value = "";
  pending.value = false;
  error.value = "";
}, { flush: "sync" });
onScopeDispose(() => { ++generation; });

async function manageBilling(): Promise<void> {
  if (billingPending.value) return;
  const version = generation;
  billingPending.value = true;
  billingError.value = "";
  try {
    const { url } = await api.createPortalSession();
    if (version === generation) props.redirect(url);
  } catch (caught: unknown) {
    if (version === generation) billingError.value = caught instanceof Error ? caught.message : "Could not open billing management.";
  } finally {
    if (version === generation) billingPending.value = false;
  }
}

async function deleteAccount(): Promise<void> {
  if (confirmation.value !== "DELETE") return;
  const version = generation;
  pending.value = true;
  error.value = "";
  try {
    await api.deleteAccount();
    if (version === generation) await api.signOut();
  } catch (caught: unknown) {
    if (version !== generation) return;
    error.value = caught instanceof Error ? caught.message : "Could not delete your account.";
    pending.value = false;
  }
}
</script>

<template>
  <section class="card">
    <p class="eyebrow">Your account</p>
    <h1>{{ session.user.value?.email }}</h1>
    <CloudAccessStatus />
    <template v-if="session.entitlementReady.value">
      <p v-if="source === 'comp'">This account has complimentary access. No purchase is needed. Contact support for questions about your grant.</p>
      <p v-else-if="source === 'app_store'">This account’s access record is from the App Store. Manage that subscription in <a href="https://apps.apple.com/account/subscriptions">Apple’s subscription settings</a>.</p>
      <p v-else-if="source && source !== 'stripe'">Access source: {{ source }}. Contact support for questions about this access.</p>
      <template v-if="hasStripe">
        <p>Manage your Stripe subscription, payment method and invoices securely on Stripe. Ordinary billing changes keep your Kanna account and data.</p>
        <button :disabled="billingPending" type="button" @click="manageBilling">{{ billingPending ? "Opening billing…" : "Manage billing" }}</button>
        <p v-if="billingError" class="error" role="alert">{{ billingError }}</p>
      </template>
      <RouterLink v-if="session.canSubscribe.value" class="button" to="/subscribe">Choose a plan</RouterLink>
    </template>
  </section>
  <SetupHandoff v-if="session.subscribed.value" />
  <section class="card danger-zone">
    <p class="eyebrow danger-text">Danger zone</p>
    <h2>Delete account</h2>
    <p>Permanently remove your Kanna Cloud account.</p>
    <button v-if="!deleting" class="danger-button" type="button" @click="deleting = true">Delete account</button>
    <form v-else class="delete-confirmation" @submit.prevent="deleteAccount">
      <p>Your subscription is canceled immediately. Your cloud data and cloud desktop pairings are permanently deleted. Local Kanna data and LAN pairings remain. There is no undo.</p>
      <label>Type DELETE to continue <input v-model="confirmation" autocomplete="off" /></label>
      <p v-if="error" class="error" role="alert">{{ error }}</p>
      <div class="confirmation-actions">
        <button class="link-button" :disabled="pending" type="button" @click="deleting = false">Cancel</button>
        <button class="danger-button" :disabled="confirmation !== 'DELETE' || pending" type="submit">{{ pending ? "Deleting…" : "Delete permanently" }}</button>
      </div>
    </form>
  </section>
</template>
