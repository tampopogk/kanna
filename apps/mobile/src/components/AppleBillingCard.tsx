import React from "react";
import { Linking, Pressable, StyleSheet, Text, View } from "react-native";
import type { AppleBillingView } from "../lib/billing/useAppleBilling";
import { MOBILE_E2E_IDS } from "../e2eTestIds";

export function AppleBillingCard({ value, verified }: { value: AppleBillingView; verified: boolean }) {
  const { billing, purchase } = value;
  const apple = billing.sources.find(s => s.source === "app_store");
  const stripe = billing.sources.find(s => s.source === "stripe");
  const comp = billing.sources.some(s => s.source === "comp" && s.active);
  const paid = billing.sources.filter(s => s.status === "active" || s.status === "grace");
  const blocked = comp || paid.length > 0 || apple?.paymentOutstanding;
  return <View style={styles.card} testID={MOBILE_E2E_IDS.appleBillingCard}>
    <Text style={styles.heading}>Kanna Cloud</Text>
    <Text style={styles.text}>Connect to your desktops away from home, browse your cloud task index, and control remote tasks.</Text>
    {!billing.ready && <Text style={styles.text} testID={MOBILE_E2E_IDS.appleBillingUnconfirmed}>Billing could not be confirmed. Refresh your account before purchasing.</Text>}
    {comp && <Text style={styles.text} testID={MOBILE_E2E_IDS.appleBillingSource("comp")}>Complimentary access. No purchase needed.</Text>}
    {stripe && <Text style={styles.text} testID={MOBILE_E2E_IDS.appleBillingSource("stripe")}>Managed outside the App Store · {stripe.status}</Text>}
    {apple && <><Text style={styles.text} testID={MOBILE_E2E_IDS.appleBillingSource("app_store")}>Apple App Store · {apple.status}{apple.environment === "sandbox" ? " (sandbox)" : ""}
      {apple.cancelAtPeriodEnd ? " · renewal off" : ""}</Text>
      <Pressable accessibilityRole="button" disabled={purchase.busy} onPress={value.manage}><Text style={styles.link}>Manage Apple subscription</Text></Pressable>
      <Text style={styles.text}>Apple handles App Store refund requests.</Text>
      <Pressable accessibilityRole="link" onPress={() => void Linking.openURL("https://reportaproblem.apple.com")}><Text style={styles.link}>Request an Apple refund</Text></Pressable>
    </>}
    {paid.length > 1 && <Text accessibilityRole="alert" style={styles.text}>You have two paid subscriptions. Manage each with its billing provider to avoid paying twice. Kanna does not automatically cancel either.</Text>}
    {billing.ready && !blocked && <>
      <Text style={styles.text} testID={MOBILE_E2E_IDS.appleBillingPrice}>{purchase.price ? `${purchase.price} per month` : "Monthly price unavailable"}</Text>
      <Text style={styles.text} testID={MOBILE_E2E_IDS.appleBillingTerms}>Payment is charged to your Apple Account. Automatically renews monthly unless canceled at least 24 hours before the period ends. Manage or cancel in Apple subscription settings.</Text>
      <Pressable accessibilityLabel="Subscribe to Kanna Cloud" accessibilityRole="button" testID={MOBILE_E2E_IDS.appleBillingSubscribeButton}
        disabled={!verified || !purchase.ready || !purchase.price || purchase.busy || purchase.pending} onPress={value.buy}
        style={[styles.subscribeButton, (!verified || !purchase.ready || !purchase.price || purchase.busy || purchase.pending) ? styles.subscribeButtonDisabled : null]}>
        <Text style={styles.subscribeLabel}>{purchase.busy ? "Working…" : "Subscribe to Kanna Cloud"}</Text>
      </Pressable>
    </>}
    <Pressable accessibilityRole="button" disabled={!verified || !purchase.ready || purchase.busy} testID={MOBILE_E2E_IDS.appleBillingRestoreButton} onPress={value.restore}><Text style={styles.link}>Restore Purchases</Text></Pressable>
    {purchase.message ? <Text accessibilityRole="alert" style={styles.text} testID={MOBILE_E2E_IDS.appleBillingMessage}>{purchase.message}</Text> : null}
    <Pressable accessibilityRole="link" testID={MOBILE_E2E_IDS.appleBillingEulaLink} onPress={() => void Linking.openURL("https://www.apple.com/legal/internet-services/itunes/dev/stdeula/")}><Text style={styles.link}>Apple Standard EULA</Text></Pressable>
    <Pressable accessibilityRole="link" testID={MOBILE_E2E_IDS.appleBillingPrivacyLink} onPress={() => void Linking.openURL("https://kanna.build/privacy")}><Text style={styles.link}>Privacy Policy</Text></Pressable>
  </View>;
}
const styles = StyleSheet.create({ card: { gap: 12 }, heading: { color: "#F4F7FF", fontSize: 18, fontWeight: "600" },
  text: { color: "#AFC0D9", lineHeight: 20 }, link: { color: "#8CB8FF", paddingVertical: 8 },
  // The purchase call to action is the one control on this card that earns
  // revenue; it is styled as the sheet's primary button, not as a text link.
  subscribeButton: { alignItems: "center", backgroundColor: "#E8F1FF", borderRadius: 16, marginTop: 4, paddingVertical: 14 },
  subscribeButtonDisabled: { opacity: 0.5 },
  subscribeLabel: { color: "#0B1220", fontSize: 15, fontWeight: "800" } });
