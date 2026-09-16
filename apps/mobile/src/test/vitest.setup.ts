import { vi } from "vitest";

/**
 * Native-only packages eventually load React Native's Flow entrypoint when
 * Vitest executes them in plain Node. Component suites provide focused mocks
 * for the native surfaces they exercise; this shared boundary supplies the
 * neutral safe-area value used by suites that do not vary device insets.
 */
vi.mock("react-native-safe-area-context", () => ({
  useSafeAreaInsets: () => ({ bottom: 0, left: 0, right: 0, top: 0 }),
}));
