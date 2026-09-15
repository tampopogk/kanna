const { registerRootComponent } = require("expo");

const {
  installMobileCrashHandler
} = require("./src/lib/diagnostics/mobileCrashDiagnostics");

installMobileCrashHandler();

const App = __DEV__ && process.env.EXPO_PUBLIC_KANNA_STOREKIT_TEST === "1"
  ? require("./e2e/storekit/StoreKitTestApp").default
  : require("./App").default;

registerRootComponent(App);
