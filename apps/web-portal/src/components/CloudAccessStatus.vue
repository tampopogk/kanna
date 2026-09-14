<script setup lang="ts">
import { usePortalSession } from "../session";
const session = usePortalSession();
</script>

<template>
  <div role="status">
    <div class="status-row">
      <span>Cloud access</span>
      <strong :class="session.subscribed.value ? 'active' : 'inactive'">{{ session.accessState.value }}</strong>
    </div>
    <template v-if="session.accessState.value === 'read-failure'">
      <p>We could not confirm cloud access. Check your connection and try again.</p>
      <button type="button" @click="session.refreshEntitlement">Check access again</button>
    </template>
    <p v-else-if="session.accessState.value === 'pending'">Cloud access has not been confirmed yet. After checkout, payment updates may take a moment. This page updates automatically. If access stays pending, check the account email above and contact support before starting another checkout.</p>
    <p v-else-if="session.accessState.value === 'active'">Your Kanna Cloud access is active.</p>
    <p v-else-if="session.accessState.value === 'grace'">Cloud access is temporarily available during billing recovery. Review your billing details.</p>
    <p v-else>Cloud access has ended. Review billing or access details on your account page.</p>
  </div>
</template>
