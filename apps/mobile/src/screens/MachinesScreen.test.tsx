import { beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { Alert } from "react-native";

vi.mock("react-native", () => ({
  Alert: { alert: vi.fn() },
  Pressable: "Pressable",
  ScrollView: "ScrollView",
  StyleSheet: { create: <T,>(styles: T) => styles },
  Text: "Text",
  View: "View"
}));

vi.mock("../components/MachinePairingSheet", () => ({
  MachinePairingSheet: "MachinePairingSheet"
}));

interface Node {
  type: unknown;
  props?: { children?: Child | Child[]; [key: string]: any };
}
type Child = Node | string | null | undefined | false;

function textContent(node: Child | Child[]): string {
  if (!node) return "";
  if (typeof node === "string") return node;
  if (Array.isArray(node)) return node.map(textContent).join("");
  if (typeof node.type === "function") {
    return textContent(node.type(node.props ?? {}));
  }
  return textContent(node.props?.children ?? []);
}

function findByTestId(node: Child | Child[], testID: string): Node | null {
  if (!node || typeof node === "string") return null;
  if (Array.isArray(node)) {
    for (const child of node) {
      const found = findByTestId(child, testID);
      if (found) return found;
    }
    return null;
  }
  if (typeof node.type === "function") {
    return findByTestId(node.type(node.props ?? {}), testID);
  }
  if (node.props?.testID === testID) return node;
  return findByTestId(node.props?.children ?? [], testID);
}

let MachinesScreen: typeof import("./MachinesScreen").MachinesScreen;

beforeAll(async () => {
  MachinesScreen = (await import("./MachinesScreen")).MachinesScreen;
});

beforeEach(() => {
  vi.mocked(Alert.alert).mockClear();
});

describe("MachinesScreen", () => {
  it("explains local QR pairing and keeps cloud access optional when empty", () => {
    const tree = MachinesScreen({
      machines: [],
      sourceWarnings: { account: null, local: null },
      pairingVisible: false,
      onBack: vi.fn(),
      onOpenPairing: vi.fn(),
      onClosePairing: vi.fn(),
      onPairCode: vi.fn(async () => undefined),
      onPairPayload: vi.fn(async () => undefined),
      onForgetMachine: vi.fn(async () => undefined)
    }) as Node;

    expect(textContent(tree)).toContain("Install Kanna for macOS from kanna.build");
    expect(textContent(tree)).toContain("tap Add and scan its pairing QR code");
    expect(textContent(tree)).toContain("connect over your local network");
    expect(textContent(tree)).toContain(
      "Cloud sign-in for remote access is separate and optional."
    );
    expect(findByTestId(tree, "mobile.machines-add")?.props).toMatchObject({
      accessibilityLabel: "Add machine"
    });
  });

  it("groups deduplicated machines and offers no removal for an account row while signed out", () => {
    const tree = MachinesScreen({
      machines: [
        {
          desktopId: "desktop-dual",
          displayName: "Jerome’s MacBook Pro",
          origins: { account: true, manual: true },
          availability: { lan: true, cloud: true, lastSeenAt: null },
          lanEndpoints: []
        },
        {
          desktopId: "desktop-account",
          displayName: "Account Mac",
          origins: { account: true, manual: false },
          availability: { lan: false, cloud: false, lastSeenAt: null },
          lanEndpoints: []
        }
      ],
      sourceWarnings: { account: null, local: null },
      pairingVisible: false,
      onBack: vi.fn(),
      onOpenPairing: vi.fn(),
      onClosePairing: vi.fn(),
      onPairCode: vi.fn(async () => undefined),
      onPairPayload: vi.fn(async () => undefined),
      onForgetMachine: vi.fn(async () => undefined)
    }) as Node;

    expect(textContent(tree)).toContain("Available");
    expect(textContent(tree)).toContain("Offline");
    expect(textContent(tree)).toContain("Account");
    expect(textContent(tree)).toContain("Paired");
    expect(textContent(tree).match(/Jerome’s MacBook Pro/g)).toHaveLength(1);
    expect(findByTestId(tree, "mobile.machine.desktop-dual.remove")).not.toBeNull();
    expect(findByTestId(tree, "mobile.machine.desktop-dual.name")).not.toBeNull();
    expect(findByTestId(tree, "mobile.machine.desktop-dual.origin.account")).not.toBeNull();
    expect(findByTestId(tree, "mobile.machine.desktop-dual.origin.manual")).not.toBeNull();
    expect(findByTestId(tree, "mobile.machine.desktop-account.remove")).toBeNull();
    expect(findByTestId(tree, "mobile.machines-back")).not.toBeNull();
    expect(findByTestId(tree, "mobile.machines-add")).not.toBeNull();
    expect(findByTestId(tree, "mobile.machines-back")?.props).toMatchObject({
      accessibilityLabel: "Back",
      accessibilityRole: "button"
    });
    expect(findByTestId(tree, "mobile.machines-add")?.props).toMatchObject({
      accessibilityLabel: "Add machine",
      accessibilityRole: "button"
    });
    expect(findByTestId(tree, "mobile.machine.desktop-dual.remove")?.props).toMatchObject({
      accessibilityLabel: "Remove Jerome’s MacBook Pro",
      accessibilityRole: "button"
    });
  });

  it("shows source warnings without hiding cached machine rows", () => {
    const tree = MachinesScreen({
      machines: [{
        desktopId: "desktop-cached",
        displayName: "Cached Mac",
        origins: { account: true, manual: false },
        availability: { lan: false, cloud: false, lastSeenAt: null },
        lanEndpoints: []
      }],
      sourceWarnings: { account: "Cloud unavailable", local: "LAN unavailable" },
      pairingVisible: false,
      onBack: vi.fn(),
      onOpenPairing: vi.fn(),
      onClosePairing: vi.fn(),
      onPairCode: vi.fn(async () => undefined),
      onPairPayload: vi.fn(async () => undefined),
      onForgetMachine: vi.fn(async () => undefined)
    }) as Node;

    expect(textContent(tree)).toContain("Cloud unavailable");
    expect(textContent(tree)).toContain("LAN unavailable");
    expect(textContent(tree)).toContain("Cached Mac");
  });

  it("names the machine in the removal confirmation and says what is deleted", () => {
    const tree = MachinesScreen({
      machines: [
        {
          desktopId: "desktop-manual",
          displayName: "Studio Mac",
          origins: { account: false, manual: true },
          availability: { lan: false, cloud: false, lastSeenAt: null },
          lanEndpoints: []
        },
        {
          desktopId: "desktop-dual",
          displayName: "Laptop",
          origins: { account: true, manual: true },
          availability: { lan: false, cloud: false, lastSeenAt: null },
          lanEndpoints: []
        }
      ],
      sourceWarnings: { account: null, local: null },
      pairingVisible: false,
      onBack: vi.fn(),
      onOpenPairing: vi.fn(),
      onClosePairing: vi.fn(),
      onPairCode: vi.fn(async () => undefined),
      onPairPayload: vi.fn(async () => undefined),
      onForgetMachine: vi.fn(async () => undefined)
    }) as Node;

    findByTestId(tree, "mobile.machine.desktop-manual.remove")?.props?.onPress?.();
    const [title, message] = vi.mocked(Alert.alert).mock.calls[0] ?? [];
    expect(title).toBe("Remove Studio Mac?");
    expect(message).toContain("Studio Mac");
    expect(message).toContain("device secret");
    expect(message).toContain("pinned identity");
    expect(message).toContain("notification pairing");
    expect(message).not.toContain("stays listed through your account");

    findByTestId(tree, "mobile.machine.desktop-dual.remove")?.props?.onPress?.();
    const [accountTitle, accountMessage] = vi.mocked(Alert.alert).mock.calls[1] ?? [];
    expect(accountTitle).toBe("Remove Laptop?");
    expect(accountMessage).toContain("device secret");
    // Signed out, the account half cannot be touched, so the copy promises
    // only what removal can actually deliver.
    expect(accountMessage).toContain("Laptop stays listed through your account");
  });

  it("offers the pairing and the account entry separately for a machine that has both", () => {
    const onForgetMachine = vi.fn(async () => undefined);
    const tree = MachinesScreen({
      machines: [{
        desktopId: "desktop-dual",
        displayName: "Studio Mac",
        origins: { account: true, manual: true },
        availability: { lan: true, cloud: true, lastSeenAt: null },
        lanEndpoints: []
      }],
      sourceWarnings: { account: null, local: null },
      pairingVisible: false,
      accountRemovalAvailable: true,
      onBack: vi.fn(),
      onOpenPairing: vi.fn(),
      onClosePairing: vi.fn(),
      onPairCode: vi.fn(async () => undefined),
      onPairPayload: vi.fn(async () => undefined),
      onForgetMachine
    }) as Node;

    findByTestId(tree, "mobile.machine.desktop-dual.remove")?.props?.onPress?.();
    const [, message, buttons] = vi.mocked(Alert.alert).mock.calls[0] ?? [];
    expect(buttons?.map((button) => button.text)).toEqual([
      "Cancel",
      "Remove pairing only",
      "Remove everywhere"
    ]);
    expect(message).toContain("“Remove everywhere” also does this");

    // Dropping the pairing while keeping cloud access stays possible.
    buttons?.find((button) => button.text === "Remove pairing only")?.onPress?.();
    expect(onForgetMachine).toHaveBeenLastCalledWith("desktop-dual", "pairing");

    buttons?.find((button) => button.text === "Remove everywhere")?.onPress?.();
    expect(onForgetMachine).toHaveBeenLastCalledWith("desktop-dual", "machine");
  });

  it("offers removal for an account-only machine once the account can be edited", () => {
    const machines = [{
      desktopId: "desktop-dev",
      displayName: "Dead Dev Instance",
      origins: { account: true, manual: false },
      availability: { lan: false, cloud: false, lastSeenAt: null },
      lanEndpoints: []
    }];
    const props = {
      machines,
      sourceWarnings: { account: null, local: null },
      pairingVisible: false,
      onBack: vi.fn(),
      onOpenPairing: vi.fn(),
      onClosePairing: vi.fn(),
      onPairCode: vi.fn(async () => undefined),
      onPairPayload: vi.fn(async () => undefined),
      onForgetMachine: vi.fn(async () => undefined)
    };

    const signedOut = MachinesScreen(props) as Node;
    expect(findByTestId(signedOut, "mobile.machine.desktop-dev.remove")).toBeNull();

    const signedIn = MachinesScreen({ ...props, accountRemovalAvailable: true }) as Node;
    const remove = findByTestId(signedIn, "mobile.machine.desktop-dev.remove");
    expect(remove).not.toBeNull();

    remove?.props?.onPress?.();
    const [title, message] = vi.mocked(Alert.alert).mock.calls[0] ?? [];
    expect(title).toBe("Remove Dead Dev Instance?");
    expect(message).toContain("removed from your account on every device");
    // Nothing is paired here, so nothing claims a device secret is deleted.
    expect(message).not.toContain("device secret");

    vi.mocked(Alert.alert).mock.calls[0]?.[2]
      ?.find((button) => button.text === "Remove")?.onPress?.();
    expect(props.onForgetMachine).toHaveBeenCalledWith("desktop-dev", "machine");
  });

  it("reports a failed manual removal instead of discarding the error", async () => {
    const removalError = new Error("Secure storage is unavailable");
    const tree = MachinesScreen({
      machines: [{
        desktopId: "desktop-manual",
        displayName: "Paired Mac",
        origins: { account: false, manual: true },
        availability: { lan: false, cloud: false, lastSeenAt: null },
        lanEndpoints: []
      }],
      sourceWarnings: { account: null, local: null },
      pairingVisible: false,
      onBack: vi.fn(),
      onOpenPairing: vi.fn(),
      onClosePairing: vi.fn(),
      onPairCode: vi.fn(async () => undefined),
      onPairPayload: vi.fn(async () => undefined),
      onForgetMachine: vi.fn(async () => Promise.reject(removalError))
    }) as Node;

    findByTestId(tree, "mobile.machine.desktop-manual.remove")?.props?.onPress?.();
    const confirmationButtons = vi.mocked(Alert.alert).mock.calls[0]?.[2];
    confirmationButtons?.find((button) => button.text === "Remove")?.onPress?.();

    await vi.waitFor(() => {
      expect(Alert.alert).toHaveBeenLastCalledWith(
        "Couldn’t remove machine",
        "Secure storage is unavailable"
      );
    });
  });
});
