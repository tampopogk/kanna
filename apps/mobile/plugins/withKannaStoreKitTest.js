const fs = require("node:fs");
const path = require("node:path");
const { withXcodeProject, IOSConfig } = require("expo/config-plugins");

// SKTestSession provides the local configuration when simctl/Appium launches
// the app; an Xcode Run-scheme reference alone does not affect simctl launches.
module.exports = function withKannaStoreKitTest(config) {
  if (config.ios?.bundleIdentifier !== "build.kanna.app.dev") {
    throw new Error("StoreKit test instrumentation is restricted to the dev bundle");
  }
  return withXcodeProject(config, config => {
    const project = config.modResults;
    const target = project.getFirstTarget().uuid;
    const appDir = config.modRequest.projectName;
    const group = project.findPBXGroupKey({ name: appDir }) || project.findPBXGroupKey({ path: appDir });
    if (!group) throw new Error("Generated app group missing for StoreKit test");
    for (const file of ["KannaStoreKitTest.m", "KannaCloud.storekit"]) {
      fs.copyFileSync(path.join(config.modRequest.projectRoot, "e2e/storekit", file),
        path.join(config.modRequest.platformProjectRoot, appDir, file));
      const relative = `${appDir}/${file}`;
      if (!project.hasFile(relative)) {
        if (file.endsWith(".m")) project.addSourceFile(relative, { target }, group);
        else IOSConfig.XcodeUtils.addResourceFileToGroup({ filepath: relative, groupName: appDir, project, isBuildFile: true, targetUuid: target });
      }
    }
    // Developer framework, linked only by the explicitly instrumented target.
    for (const item of Object.values(project.pbxXCBuildConfigurationSection())) {
      if (!item || typeof item !== "object" || !item.buildSettings?.PRODUCT_NAME || item.name !== "Debug") continue;
      item.buildSettings['"FRAMEWORK_SEARCH_PATHS[sdk=iphonesimulator*]"'] = '"$(inherited) $(SDKROOT)/Developer/Library/Frameworks"';
      item.buildSettings['"LD_RUNPATH_SEARCH_PATHS[sdk=iphonesimulator*]"'] = '"$(inherited) $(PLATFORM_DIR)/Developer/Library/Frameworks $(PLATFORM_DIR)/Developer/usr/lib"';
      const previous = item.buildSettings.OTHER_LDFLAGS || '"$(inherited)"';
      item.buildSettings.OTHER_LDFLAGS = Array.isArray(previous)
        ? [...previous, '"-framework"', '"StoreKitTest"']
        : [previous, '"-framework"', '"StoreKitTest"'];
    }
    return config;
  });
};
