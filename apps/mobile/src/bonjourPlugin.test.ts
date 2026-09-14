import { createRequire } from "node:module";
import { describe, expect, it } from "vitest";

const require = createRequire(import.meta.url);
const { __internal } = require("../plugins/withKannaBonjour.js");

describe("withKannaBonjour internals", () => {
  it("writes App Store-safe local network permission metadata", () => {
    const plist = __internal.applyInfoPlist({});

    expect(plist.NSBonjourServices).toEqual(["_kanna-mobile._tcp"]);
    expect(plist.NSLocalNetworkUsageDescription).toBe(
      "Kanna uses your local network to find and connect to your paired Kanna desktop app."
    );
  });

  it("deduplicates the Bonjour service and replaces stale permission copy", () => {
    const plist = __internal.applyInfoPlist({
      NSBonjourServices: ["_kanna-mobile._tcp", "_example._tcp"],
      NSLocalNetworkUsageDescription: "Old copy"
    });

    expect(plist.NSBonjourServices).toEqual([
      "_kanna-mobile._tcp",
      "_example._tcp"
    ]);
    expect(plist.NSLocalNetworkUsageDescription).toBe(
      "Kanna uses your local network to find and connect to your paired Kanna desktop app."
    );
  });

  it("patches the SDK AppDelegate with the physical-device Metro endpoint", () => {
    const appDelegate = `import Expo
import React
import ReactAppDependencyProvider

@main
class AppDelegate: ExpoAppDelegate {
  override func bundleURL() -> URL? {
#if DEBUG
    return RCTBundleURLProvider.sharedSettings().jsBundleURL(forBundleRoot: ".expo/.virtual-metro-entry")
#else
    return Bundle.main.url(forResource: "main", withExtension: "jsbundle")
#endif
  }
}
`;

    const patched = __internal.patchAppDelegate(appDelegate);

    expect(patched).toContain("return kannaMetroBundleURL()");
    expect(patched).toContain('let host = readBundledTextResource("ip")');
    expect(patched).toContain('let port = readBundledTextResource("metro-port") ?? "8081"');
    expect(__internal.patchAppDelegate(patched)).toBe(patched);
  });

  it("fails when the Expo AppDelegate template no longer matches", () => {
    expect(() => __internal.patchAppDelegate("class AppDelegate: ExpoAppDelegate {}"))
      .toThrow("Unsupported Expo AppDelegate template");
  });

  it("adds the Metro port resource step to the React Native bundle phase once", () => {
    const project = {
      hash: {
        project: {
          objects: {
            PBXShellScriptBuildPhase: {
              bundlePhase: {
                name: '"Bundle React Native code and images"',
                shellScript:
                  'set -e\nexport PROJECT_ROOT=\\"$PROJECT_DIR\\"/..\\n\\n/bin/sh `node --print "require(\'react-native/package.json\').bin"`\n'
              }
            }
          }
        }
      }
    };

    __internal.patchMetroPortScript(project);
    const once = project.hash.project.objects.PBXShellScriptBuildPhase.bundlePhase.shellScript;
    __internal.patchMetroPortScript(project);

    expect(once).toContain('echo \\"${RCT_METRO_PORT:-8081}\\" >');
    expect(project.hash.project.objects.PBXShellScriptBuildPhase.bundlePhase.shellScript).toBe(once);
  });

  it("fails when the React Native bundle phase no longer matches", () => {
    const project = {
      hash: {
        project: {
          objects: {
            PBXShellScriptBuildPhase: {}
          }
        }
      }
    };

    expect(() => __internal.patchMetroPortScript(project))
      .toThrow("Unsupported React Native bundle phase template");
  });
});

describe("withKannaBonjour Android", () => {
  const packageName = "build.kanna.app.staging";

  it("generates a module and package in the app's own Kotlin package", () => {
    const module = __internal.androidModuleSource(packageName);
    const compat = __internal.androidNsdCompatSource(packageName);
    const pkg = __internal.androidPackageSource(packageName);

    expect(module.startsWith(`package ${packageName}\n`)).toBe(true);
    expect(module).toContain("class KannaBonjourModule(reactContext: ReactApplicationContext)");
    expect(module).toContain('private const val DISCOVERY_SERVICE_TYPE = "_kanna-mobile._tcp"');
    expect(module).toContain('private const val EVENT_SERVICE_TYPE = "_kanna-mobile._tcp."');
    expect(module).toContain('private const val EVENT_NAME = "kannaBonjourServiceChanged"');
    // The readiness surface the pairing flow awaits, and the lifecycle that
    // keeps exactly one browse alive while the app is foregrounded.
    expect(module).toContain("fun ensureBrowsing(promise: Promise)");
    expect(module).toContain("override fun onHostPause()");
    expect(module).toContain("override fun onHostResume()");
    expect(module).toContain("NsdManager.PROTOCOL_DNS_SD");
    // A numeric address would be blocked by the scoped cleartext policy.
    expect(module).toContain("normalizeHostname(KannaNsdCompat.hostname(serviceInfo))");
    expect(module).not.toContain("serviceInfo.hostname");

    expect(compat).toContain("Build.VERSION.SDK_INT >= HOSTNAME_SDK");
    expect(compat).toContain("Build.VERSION_CODES.TIRAMISU");
    expect(compat).toContain("HOSTNAME_T_EXTENSION = 17");
    expect(compat).toContain("catch (LinkageError error)");
    expect(compat).not.toContain("SERVICE_INFO_CALLBACK_SDK");

    expect(pkg.startsWith(`package ${packageName}\n`)).toBe(true);
    expect(pkg).toContain("listOf(KannaBonjourModule(reactContext))");
  });

  // Verbatim shape of the Expo SDK 57 / React Native 0.86 template.
  it("registers the package in the generated MainApplication exactly once", () => {
    const mainApplication = `package ${packageName}

class MainApplication : Application(), ReactApplication {

  override val reactHost: ReactHost by lazy {
    ExpoReactHostFactory.getDefaultReactHost(
      context = applicationContext,
      packageList =
        PackageList(this).packages.apply {
          // Packages that cannot be autolinked yet can be added manually here, for example:
          // add(MyReactNativePackage())
        }
    )
  }
}
`;

    const patched = __internal.patchMainApplication(mainApplication);

    expect(patched).toContain("PackageList(this).packages.apply {\n          add(KannaBonjourPackage())");
    expect(__internal.patchMainApplication(patched)).toBe(patched);
  });

  it("registers the package in a MainApplication that returns the list directly", () => {
    const mainApplication = `package ${packageName}

override fun getPackages(): List<ReactPackage> {
  return PackageList(this).packages
}
`;

    const patched = __internal.patchMainApplication(mainApplication);

    expect(patched).toContain("add(KannaBonjourPackage())");
    expect(__internal.patchMainApplication(patched)).toBe(patched);
  });

  it("registers the package in a MainApplication that builds a package list", () => {
    const mainApplication = `package ${packageName}

override fun getPackages(): List<ReactPackage> {
  val packages = PackageList(this).packages
  return packages
}
`;

    const patched = __internal.patchMainApplication(mainApplication);

    expect(patched).toContain("packages.add(KannaBonjourPackage())");
    expect(patched.indexOf("packages.add(KannaBonjourPackage())"))
      .toBeLessThan(patched.indexOf("return packages"));
    expect(__internal.patchMainApplication(patched)).toBe(patched);
  });

  it("fails when the Expo MainApplication template no longer matches", () => {
    expect(() => __internal.patchMainApplication("class MainApplication {}"))
      .toThrow("Unsupported Expo MainApplication template");
  });

  it("permits LAN cleartext for mDNS names only", () => {
    const xml = __internal.NETWORK_SECURITY_CONFIG_XML;

    expect(xml).toContain('<base-config cleartextTrafficPermitted="false" />');
    expect(xml).toContain('<domain includeSubdomains="true">local</domain>');
    expect(xml).not.toContain('<base-config cleartextTrafficPermitted="true"');
  });

  it("points the shipped manifest at the scoped network security config", () => {
    const manifest = {
      manifest: {
        application: [{ $: { "android:name": ".MainApplication" } }]
      }
    };

    const patched = __internal.applyAndroidManifest(manifest);

    expect(patched.manifest.application[0].$["android:networkSecurityConfig"])
      .toBe("@xml/kanna_network_security_config");
  });

  it("leaves the iOS plist and AppDelegate behavior untouched", () => {
    // The Android slice must not disturb the shipped iOS discovery contract.
    expect(__internal.applyInfoPlist({}).NSBonjourServices).toEqual([
      "_kanna-mobile._tcp"
    ]);
    expect(__internal.androidModuleSource(packageName)).not.toContain("NetServiceBrowser");
  });
});
