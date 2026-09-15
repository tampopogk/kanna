import { getApp } from "firebase/app";
import { connectFunctionsEmulator, getFunctions, type Functions } from "firebase/functions";
import { readExpoFirebaseEnv } from "./config";

const connected = new WeakSet<Functions>();
export function getConfiguredFunctions(): Functions {
  const functions = getFunctions(getApp(), "us-central1");
  const env = readExpoFirebaseEnv();
  const host = env.EXPO_PUBLIC_FIREBASE_FUNCTIONS_EMULATOR_HOST?.trim();
  const port = Number(env.EXPO_PUBLIC_FIREBASE_FUNCTIONS_EMULATOR_PORT);
  if (host && Number.isInteger(port) && port > 0 && port <= 65_535 && !connected.has(functions)) {
    connectFunctionsEmulator(functions, host, port);
    connected.add(functions);
  }
  return functions;
}
