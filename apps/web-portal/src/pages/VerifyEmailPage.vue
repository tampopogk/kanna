<script setup lang="ts">
import { onMounted, onUnmounted, ref, triggerRef, watch } from "vue";
import { useRouter } from "vue-router";
import { usePortalFirebase, usePortalSession } from "../session";

const session = usePortalSession();
const api = usePortalFirebase();
const router = useRouter();
const checking = ref(false);
const resending = ref(false);
const message = ref("");
let disposed = false;
let generation = 0;
watch(() => session.user.value?.uid, () => {
  ++generation;
  message.value = "";
  checking.value = false;
  resending.value = false;
}, { flush: "sync" });

async function checkVerification(showUnverifiedMessage = true): Promise<void> {
  const current = session.user.value;
  if (!current || checking.value) return;
  const version = generation;
  checking.value = true;
  try {
    const user = await api.reloadUser(current);
    if (disposed || version !== generation || session.user.value !== current) return;
    session.user.value = user;
    triggerRef(session.user);
    if (user.emailVerified) {
      await router.push(session.subscribed.value ? "/account" : "/subscribe");
    } else if (showUnverifiedMessage) {
      message.value = "That address is not verified yet. Open the latest link in your email, then try again.";
    }
  } catch (caught: unknown) {
    if (!disposed && version === generation && session.user.value === current) {
      message.value = caught instanceof Error ? caught.message : "Could not check verification. Please try again.";
    }
  } finally {
    if (version === generation) checking.value = false;
  }
}

async function resend(): Promise<void> {
  const current = session.user.value;
  if (!current || resending.value) return;
  const version = generation;
  resending.value = true;
  message.value = "";
  try {
    await api.resendVerification(current);
    if (!disposed && version === generation && session.user.value === current) message.value = "Verification email sent. Check your inbox and spam folder, and use the latest link.";
  } catch (caught: unknown) {
    if (!disposed && version === generation && session.user.value === current) message.value = caught instanceof Error ? caught.message : "Could not resend verification. Please try again.";
  } finally {
    if (version === generation) resending.value = false;
  }
}

function checkAfterFocus(): void { void checkVerification(false); }
onMounted(() => { window.addEventListener("focus", checkAfterFocus); });
onUnmounted(() => {
  disposed = true;
  window.removeEventListener("focus", checkAfterFocus);
});
</script>

<template>
  <section class="card narrow">
    <p class="eyebrow">One more step</p>
    <h1>Verify your email</h1>
    <p>Open the verification email for <strong>{{ session.user.value?.email }}</strong>, then return here to continue.</p>
    <p>If the link expired or the email is missing, check your spam folder or send a new link. If the address is wrong, sign out and use the correct account.</p>
    <p v-if="message" role="status">{{ message }}</p>
    <button :disabled="checking || resending" type="button" @click="checkVerification()">{{ checking ? "Checking…" : "I verified my email" }}</button>
    <button class="link-button" :disabled="checking || resending" type="button" @click="resend">{{ resending ? "Sending…" : "Resend verification email" }}</button>
  </section>
</template>
