import React from "react";
import { Alert, Pressable, ScrollView, StyleSheet, Text, View } from "react-native";
import { MachinePairingSheet } from "../components/MachinePairingSheet";
import { MOBILE_E2E_IDS } from "../e2eTestIds";
import type { ForgetMachineScope } from "../state/mobileController";
import type { MobileMachine } from "../state/machineInventory";
import { secureChannelStatusLabel } from "../lib/security/secureChannelPeer";

interface MachinesScreenProps {
  machines: MobileMachine[];
  sourceWarnings: { account: string | null; local: string | null };
  pairingVisible: boolean;
  /** Short authentication string a typed-code pairing is waiting on. */
  pairingConfirmationSas?: string | null;
  onBack(): void;
  onOpenPairing(): void;
  onClosePairing(): void;
  onPairCode(code: string): Promise<void>;
  onPairPayload(payload: string): Promise<void>;
  /** Forget the machine. `scope` decides how much of it goes. */
  onForgetMachine(desktopId: string, scope: ForgetMachineScope): Promise<void>;
  /**
   * Whether the account's cloud directory can be edited right now. False when
   * signed out, which is what makes an account-only machine unremovable: its
   * entry is backend-authored and this phone holds nothing of it to delete.
   */
  accountRemovalAvailable?: boolean;
}

export function MachinesScreen(props: MachinesScreenProps) {
  const available = props.machines.filter(isAvailable);
  const offline = props.machines.filter((machine) => !isAvailable(machine));

  const accountRemovalAvailable = props.accountRemovalAvailable ?? false;
  const canRemove = (machine: MobileMachine) =>
    machine.origins.manual || (machine.origins.account && accountRemovalAvailable);

  const forget = (machine: MobileMachine, scope: ForgetMachineScope) => {
    void props.onForgetMachine(machine.desktopId, scope).catch((error) => {
      const detail = error instanceof Error
        ? error.message
        : "The machine could not be removed. Try again.";
      Alert.alert("Couldn’t remove machine", detail);
    });
  };

  // Naming the machine in the title is the whole confirmation: the rows differ
  // only by name, and removal deletes trust material that cannot be recovered
  // without pairing the machine again in person.
  //
  // A machine that is both paired and account-backed is two removals, not
  // one, and the difference is not cosmetic - dropping the pairing while
  // keeping cloud access is a thing people do on purpose - so the choice is
  // offered rather than decided here.
  const confirmRemoval = (machine: MobileMachine) => {
    const removesBoth = machine.origins.manual
      && machine.origins.account
      && accountRemovalAvailable;
    Alert.alert(
      `Remove ${machine.displayName}?`,
      removalMessage(machine, accountRemovalAvailable),
      removesBoth
        ? [
            { text: "Cancel", style: "cancel" },
            {
              text: "Remove pairing only",
              style: "destructive",
              onPress: () => forget(machine, "pairing")
            },
            {
              text: "Remove everywhere",
              style: "destructive",
              onPress: () => forget(machine, "machine")
            }
          ]
        : [
            { text: "Cancel", style: "cancel" },
            {
              text: "Remove",
              style: "destructive",
              onPress: () => forget(machine, "machine")
            }
          ]
    );
  };

  return (
    <View style={styles.screen} testID={MOBILE_E2E_IDS.machinesScreen}>
      <View style={styles.header}>
        <Pressable
          accessibilityLabel="Back"
          accessibilityRole="button"
          style={styles.headerAction}
          testID={MOBILE_E2E_IDS.machinesBackButton}
          onPress={props.onBack}
        >
          <Text style={styles.headerActionLabel}>‹ Back</Text>
        </Pressable>
        <Text style={styles.title}>Machines</Text>
        <Pressable
          accessibilityLabel="Add machine"
          accessibilityRole="button"
          style={styles.headerAction}
          testID={MOBILE_E2E_IDS.machinesAddButton}
          onPress={props.onOpenPairing}
        >
          <Text style={styles.headerActionLabel}>Add</Text>
        </Pressable>
      </View>

      <ScrollView contentContainerStyle={styles.content} showsVerticalScrollIndicator={false}>
        {props.sourceWarnings.account ? (
          <WarningBanner label="Account" message={props.sourceWarnings.account} />
        ) : null}
        {props.sourceWarnings.local ? (
          <WarningBanner label="Local network" message={props.sourceWarnings.local} />
        ) : null}

        <MachineSection
          title="Available"
          machines={available}
          canRemove={canRemove}
          onRemove={confirmRemoval}
        />
        <MachineSection
          title="Offline"
          machines={offline}
          canRemove={canRemove}
          onRemove={confirmRemoval}
        />

        {props.machines.length === 0 ? (
          <View style={styles.empty}>
            <Text style={styles.emptyTitle}>No machines added</Text>
            <Text style={styles.emptyDetail}>
              Install Kanna for macOS from kanna.build, then tap Add and scan
              its pairing QR code to connect over your local network. Cloud
              sign-in for remote access is separate and optional.
            </Text>
          </View>
        ) : null}
      </ScrollView>

      <MachinePairingSheet
        visible={props.pairingVisible}
        confirmationSas={props.pairingConfirmationSas ?? null}
        onClose={props.onClosePairing}
        onPairCode={props.onPairCode}
        onPairPayload={props.onPairPayload}
      />
    </View>
  );
}

function MachineSection({
  title,
  machines,
  canRemove,
  onRemove
}: {
  title: string;
  machines: MobileMachine[];
  canRemove(machine: MobileMachine): boolean;
  onRemove(machine: MobileMachine): void;
}) {
  if (machines.length === 0) return null;
  return (
    <View style={styles.section}>
      <Text style={styles.sectionTitle}>{title}</Text>
      {machines.map((machine) => (
        <View
          key={machine.desktopId}
          style={styles.card}
          testID={MOBILE_E2E_IDS.machineRow(machine.desktopId)}
        >
          <View style={styles.cardHeader}>
            <View style={styles.cardIdentity}>
              <Text
                style={styles.machineName}
                testID={MOBILE_E2E_IDS.machineName(machine.desktopId)}
              >
                {machine.displayName}
              </Text>
              <View style={styles.originRow}>
                {machine.origins.account ? (
                  <OriginPill
                    label="Account"
                    testID={MOBILE_E2E_IDS.machineOrigin(machine.desktopId, "account")}
                  />
                ) : null}
                {machine.origins.manual ? (
                  <OriginPill
                    label="Paired"
                    testID={MOBILE_E2E_IDS.machineOrigin(machine.desktopId, "manual")}
                  />
                ) : null}
              </View>
            </View>
            {canRemove(machine) ? (
              <Pressable
                accessibilityLabel={`Remove ${machine.displayName}`}
                accessibilityRole="button"
                testID={MOBILE_E2E_IDS.machineRemoveButton(machine.desktopId)}
                onPress={() => onRemove(machine)}
              >
                <Text style={styles.remove}>Remove</Text>
              </Pressable>
            ) : null}
          </View>
          <Text style={styles.availability}>{availabilityLabel(machine)}</Text>
          {machine.secureChannel ? (
            <Text
              style={
                machine.secureChannel.mode === "sealed"
                  ? styles.securitySealed
                  : machine.secureChannel.mode === "legacy"
                    ? styles.securityLegacy
                    : styles.securityRefused
              }
              testID={MOBILE_E2E_IDS.machineSecurity(machine.desktopId)}
            >
              {secureChannelStatusLabel(machine.secureChannel)}
            </Text>
          ) : null}
        </View>
      ))}
    </View>
  );
}

function OriginPill({ label, testID }: { label: string; testID: string }) {
  return (
    <View style={styles.pill}>
      <Text style={styles.pillLabel} testID={testID}>{label}</Text>
    </View>
  );
}

function WarningBanner({ label, message }: { label: string; message: string }) {
  return (
    <View style={styles.warning}>
      <Text style={styles.warningLabel}>{label}</Text>
      <Text style={styles.warningMessage}>{message}</Text>
    </View>
  );
}

/**
 * What removal actually destroys, per origin. A pairing is trust material this
 * phone holds and its deletion is final here; an account entry is a
 * backend-authored directory row, and a machine that is still running simply
 * republishes it, which the copy says rather than promising more than removal
 * can deliver.
 */
function removalMessage(
  machine: MobileMachine,
  accountRemovalAvailable: boolean
): string {
  const pairing =
    `This phone deletes its pairing with ${machine.displayName} — the device ` +
    "secret, the pinned identity and the notification pairing — and its tasks " +
    "disappear from this phone. Pairing again means scanning its QR code.";
  const account =
    `${machine.displayName} is removed from your account on every device. If ` +
    "the machine is still running, it will appear again the next time it " +
    "connects.";
  if (machine.origins.manual && machine.origins.account) {
    return accountRemovalAvailable
      ? `${pairing}\n\n“Remove everywhere” also does this: ${account}`
      : `${pairing}\n\n${machine.displayName} stays listed through your account.`;
  }
  return machine.origins.manual ? pairing : account;
}

function isAvailable(machine: MobileMachine): boolean {
  return machine.availability.lan || machine.availability.cloud;
}

function availabilityLabel(machine: MobileMachine): string {
  if (machine.availability.lan && machine.availability.cloud) return "Available nearby and through your account";
  if (machine.availability.lan) return "Available on this network";
  if (machine.availability.cloud) return "Available through your account";
  return machine.availability.lastSeenAt
    ? `Last seen ${new Date(machine.availability.lastSeenAt).toLocaleString()}`
    : "Offline";
}

const styles = StyleSheet.create({
  screen: { flex: 1 },
  header: { alignItems: "center", flexDirection: "row", justifyContent: "space-between", paddingBottom: 18 },
  title: { color: "#F5F7FB", fontSize: 22, fontWeight: "800" },
  headerAction: { minWidth: 64, paddingVertical: 8 },
  headerActionLabel: { color: "#9FC1F5", fontSize: 14, fontWeight: "700" },
  content: { gap: 18, paddingBottom: 120 },
  section: { gap: 10 },
  sectionTitle: { color: "#8398B7", fontSize: 12, fontWeight: "800", letterSpacing: 1, textTransform: "uppercase" },
  card: { backgroundColor: "#111B2C", borderColor: "#20304C", borderRadius: 18, borderWidth: 1, gap: 10, padding: 16 },
  cardHeader: { alignItems: "flex-start", flexDirection: "row", justifyContent: "space-between" },
  cardIdentity: { flex: 1, gap: 8 },
  machineName: { color: "#F5F7FB", fontSize: 16, fontWeight: "700" },
  originRow: { flexDirection: "row", gap: 6 },
  pill: { backgroundColor: "#172843", borderRadius: 999, paddingHorizontal: 9, paddingVertical: 4 },
  pillLabel: { color: "#9EB6DC", fontSize: 10, fontWeight: "800", textTransform: "uppercase" },
  availability: { color: "#AABAD1", fontSize: 13 },
  securitySealed: { color: "#7FD1A8", fontSize: 12, fontWeight: "700", marginTop: 6 },
  securityLegacy: { color: "#E3B34C", fontSize: 12, fontWeight: "700", marginTop: 6 },
  securityRefused: { color: "#F08A8A", fontSize: 12, fontWeight: "700", marginTop: 6 },
  remove: { color: "#FFAAA6", fontSize: 13, fontWeight: "700" },
  warning: { backgroundColor: "#2A2315", borderColor: "#5C4A23", borderRadius: 14, borderWidth: 1, gap: 3, padding: 12 },
  warningLabel: { color: "#E7C978", fontSize: 11, fontWeight: "800", textTransform: "uppercase" },
  warningMessage: { color: "#D7CDAF", fontSize: 13 },
  empty: { alignItems: "center", gap: 7, paddingHorizontal: 32, paddingVertical: 56 },
  emptyTitle: { color: "#F5F7FB", fontSize: 18, fontWeight: "800" },
  emptyDetail: { color: "#91A3BD", fontSize: 14, lineHeight: 20, textAlign: "center" }
});
