/** Local native boundary test. The fake durable acknowledgment grants no cloud
 * access and contacts no Functions endpoint. Apple trust is tested separately. */
import React, { useEffect, useState } from "react";
import { NativeModules, Platform, Text, View } from "react-native";
import * as store from "expo-iap";
import { createAppleController, type ApplePurchaseState } from "../../src/lib/billing/appleController";
import { readExpoConfig } from "../../src/lib/expoConfig";
import { readKannaExpoExtra } from "../../src/mobileEnvironment";

export default function StoreKitTestApp() {
  const [result, setResult] = useState("running");
  useEffect(() => {
    let live = true;
    let controller: ReturnType<typeof createAppleController> | undefined;
    const run = async () => {
      if (!__DEV__ || Platform.OS !== "ios" || readKannaExpoExtra(readExpoConfig())?.appEnv !== "dev" || !NativeModules.KannaStoreKitTest) {
        throw new Error("Requires the isolated, instrumented dev simulator binary");
      }
      await NativeModules.KannaStoreKitTest.start();
      let last: ApplePurchaseState | null = null;
      let waiter: (() => void) | null = null;
      const publish = (state: ApplePurchaseState) => { last = state; waiter?.(); };
      const wait = (predicate: (state: ApplePurchaseState) => boolean) => new Promise<void>((resolve, reject) => {
        const timeout = setTimeout(() => { waiter = null; reject(new Error(`StoreKit state timed out: ${last?.message}`)); }, 60_000);
        waiter = () => { if (last && predicate(last)) { clearTimeout(timeout); waiter = null; resolve(); } };
        waiter();
      });
      let release!: () => void;
      let registered!: () => void;
      const registration = new Promise<void>(resolve => { registered = resolve; });
      const accepted = new Promise<void>(resolve => { release = resolve; });
      let registrations = 0;
      const client = {
        begin: async () => ({ appAccountToken: "11111111-1111-4111-8111-111111111111", productId: "build.kanna.cloud.monthly" }),
        register: async (_uid: string, jws: string) => {
          if (jws.split(".").length !== 3) throw new Error("Native purchase did not include a JWS");
          registrations++; registered(); await accepted;
        },
      };
      const make = () => { const value = createAppleController(store, client, publish, async () => {}); value.setAccount({ uid: "local-fixture", verified: true }); return value; };
      if (NativeModules.KannaStoreKitTest.restoreOnly) {
        release(); controller = make(); await controller.start();
        await controller.restore();
        if (!registrations) throw new Error(`Relaunch restore did not recover the existing purchase: ${last?.message ?? "no transaction"}`);
        await NativeModules.KannaStoreKitTest.clear();
        if (live) setResult("passed: relaunch restore");
        return;
      }
      controller = make(); await controller.start();
      await NativeModules.KannaStoreKitTest.setPending(true);
      const pending = wait(state => state.pending);
      void controller.purchase(); await pending;
      if (registrations) throw new Error("Pending purchase was registered as paid");
      const completed = wait(state => state.message.startsWith("Purchase verified"));
      await NativeModules.KannaStoreKitTest.approve();
      let registrationTimeout: ReturnType<typeof setTimeout> | undefined;
      try {
        await Promise.race([registration, new Promise((_, reject) => { registrationTimeout = setTimeout(() => reject(new Error("No native transaction")), 60_000); })]);
      } finally { clearTimeout(registrationTimeout); }
      if ((await store.getPendingTransactionsIOS()).length === 0) throw new Error("Transaction finished before durable acknowledgment");
      release(); await completed;
      if ((await store.getPendingTransactionsIOS()).length !== 0) throw new Error("Transaction was not finished after acknowledgment");
      const count = registrations;
      await controller.restore();
      if (registrations <= count) throw new Error("Explicit native restore did not deliver the existing purchase");
      await controller.dispose(); controller = undefined;
      // Native connection restart models app-session recovery without buying.
      const restarted = make(); controller = restarted; await restarted.start();
      if (registrations <= count + 1) throw new Error("Restart did not recover the existing purchase");
      if (live) setResult("passed: native pending approval, purchase, deferred finish, restore, session restart");
    };
    void run().catch(error => { if (live) setResult(`failed: ${error instanceof Error ? error.message : String(error)}`); });
    return () => { live = false; controller?.dispose(); };
  }, []);
  return <View collapsable={false} testID="storekit-test-shell" style={{ flex: 1, justifyContent: "center", padding: 24, backgroundColor: "white" }}>
    <Text testID="storekit-test-result" style={{ color: "black" }}>{result}</Text>
  </View>;
}
