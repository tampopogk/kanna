<script setup lang="ts">
import { RouterLink } from "vue-router";
import { usePortalSession } from "../session";
import CloudAccessStatus from "../components/CloudAccessStatus.vue";
import SetupHandoff from "../components/SetupHandoff.vue";

defineProps<{ result: "success" | "canceled"; sessionId?: string | null }>();
const session = usePortalSession();
</script>

<template>
  <section class="card narrow">
    <p class="eyebrow">Back from Stripe</p>
    <h1>{{ result === 'success' ? 'Checking cloud access' : 'Checkout cancelled' }}</h1>
    <p>Signed in as <strong>{{ session.user.value?.email }}</strong>.</p>
    <p v-if="result === 'canceled'">You returned from checkout. This return does not confirm whether a payment was made; your account’s cloud access is shown below.</p>
    <CloudAccessStatus />
    <RouterLink class="button" to="/account">View account and billing</RouterLink>
    <p class="quiet">Use the same verified account you used at checkout. If this is a different account, sign out and sign in with that email.</p>
  </section>
  <SetupHandoff v-if="session.subscribed.value" />
</template>
