import { beforeEach, describe, expect, it, vi } from "vitest";

const nativeMocks = vi.hoisted(() => ({
  actionSheet: vi.fn(),
  alert: vi.fn(),
  showAlert: vi.fn(),
  platform: { OS: "ios" }
}));

vi.mock("react-native", () => ({
  ActionSheetIOS: {
    showActionSheetWithOptions: nativeMocks.actionSheet
  },
  Alert: {
    alert: nativeMocks.alert
  },
  Platform: nativeMocks.platform,
  TurboModuleRegistry: {
    get: (name: string) =>
      name === "DialogManagerAndroid"
        ? {
            // Values from React Native's DialogModule.kt.
            getConstants: () => ({
              buttonClicked: "buttonClicked",
              dismissed: "dismissed",
              buttonPositive: -1,
              buttonNegative: -2,
              buttonNeutral: -3
            }),
            showAlert: nativeMocks.showAlert
          }
        : null
  }
}));

type AndroidOnAction = (action: string, buttonKey?: number) => void;

function androidDialog() {
  const [config, onError, onAction] = nativeMocks.showAlert.mock.calls[0]! as [
    { title?: string; message?: string; items?: string[]; buttonNegative?: string; cancelable?: boolean },
    (error: string) => void,
    AndroidOnAction
  ];
  return { config, onError, onAction };
}

import { showTaskActionMenu } from "./taskActionMenu";

describe("showTaskActionMenu", () => {
  beforeEach(() => {
    nativeMocks.actionSheet.mockReset();
    nativeMocks.alert.mockReset();
    nativeMocks.showAlert.mockReset();
    nativeMocks.platform.OS = "ios";
  });

  it("shows task actions with close marked destructive", () => {
    showTaskActionMenu(
      { mentionedFilesLabel: "Mentioned Files (3)" },
      vi.fn()
    );

    expect(nativeMocks.actionSheet).toHaveBeenCalledWith(
      {
        title: "Task Actions",
        options: [
          "Browse Files",
          "Mentioned Files (3)",
          "View Diff",
          "Advance Stage",
          "Close Task",
          "Cancel"
        ],
        cancelButtonIndex: 5,
        destructiveButtonIndex: 4
      },
      expect.any(Function)
    );
  });

  it("offers only close for an unresolved task creation", () => {
    showTaskActionMenu(
      { mentionedFilesLabel: "Mentioned Files (0)", taskCreation: true },
      vi.fn()
    );

    expect(nativeMocks.actionSheet).toHaveBeenCalledWith(
      {
        title: "Task Actions",
        options: ["Close Task", "Cancel"],
        cancelButtonIndex: 1,
        destructiveButtonIndex: 0
      },
      expect.any(Function)
    );
  });

  it("offers opening an artifact by tree id when the client can read artifacts", () => {
    const onSelect = vi.fn();
    showTaskActionMenu(
      { mentionedFilesLabel: "Mentioned Files (0)", artifactsAvailable: true },
      onSelect
    );

    const [sheet, choose] = nativeMocks.actionSheet.mock.calls[0];
    expect(sheet.options).toEqual([
      "Browse Files",
      "Mentioned Files (0)",
      "View Diff",
      "Open Artifact…",
      "Advance Stage",
      "Close Task",
      "Cancel"
    ]);
    choose(3);
    expect(onSelect).toHaveBeenCalledWith("open-artifact");
  });

  it("offers dev-server preview only when a declared port is available", () => {
    showTaskActionMenu(
      {
        mentionedFilesLabel: "Mentioned Files (0)",
        previewAvailable: true
      },
      vi.fn()
    );

    expect(nativeMocks.actionSheet).toHaveBeenCalledWith(
      expect.objectContaining({
        options: [
          "Preview Dev Server",
          "Browse Files",
          "Mentioned Files (0)",
          "View Diff",
          "Advance Stage",
          "Close Task",
          "Cancel"
        ],
        cancelButtonIndex: 6,
        destructiveButtonIndex: 5
      }),
      expect.any(Function)
    );
  });

  it.each([
    [0, "browse-files"],
    [1, "mentioned-files"],
    [2, "view-diff"],
    [3, "advance-stage"],
    [4, "close-task"]
  ] as const)("maps iOS index %s to %s", (index, action) => {
    const onSelect = vi.fn();
    showTaskActionMenu({ mentionedFilesLabel: "Mentioned Files (0)" }, onSelect);
    const callback = nativeMocks.actionSheet.mock.calls[0]![1] as (
      buttonIndex: number
    ) => void;

    callback(index);

    expect(onSelect).toHaveBeenCalledOnce();
    expect(onSelect).toHaveBeenCalledWith(action);
  });

  it.each([5, 99])("ignores cancel or invalid iOS index %s", (index) => {
    const onSelect = vi.fn();
    const onDismiss = vi.fn();
    showTaskActionMenu(
      { mentionedFilesLabel: "Mentioned Files (0)" },
      onSelect,
      onDismiss
    );
    const callback = nativeMocks.actionSheet.mock.calls[0]![1] as (
      buttonIndex: number
    ) => void;

    callback(index);

    expect(onSelect).not.toHaveBeenCalled();
    expect(onDismiss).toHaveBeenCalledOnce();
  });

  it("lists every task action on Android, not just the three an Alert can hold", () => {
    nativeMocks.platform.OS = "android";
    const onSelect = vi.fn();

    showTaskActionMenu(
      {
        mentionedFilesLabel: "Mentioned Files (2)",
        previewAvailable: true,
        artifactsAvailable: true
      },
      onSelect
    );

    // Alert.alert keeps only three buttons on Android and silently drops the
    // rest, which left "Open Artifact…" and later actions unreachable.
    expect(nativeMocks.alert).not.toHaveBeenCalled();
    const { config, onAction } = androidDialog();
    expect(config).toEqual({
      title: "Task Actions",
      items: [
        "Preview Dev Server",
        "Browse Files",
        "Mentioned Files (2)",
        "View Diff",
        "Open Artifact…",
        "Advance Stage",
        "Close Task"
      ],
      buttonNegative: "Cancel",
      cancelable: true
    });
    // AlertDialog shows the message instead of the items when both are set.
    expect(config).not.toHaveProperty("message");

    config.items!.forEach((_, index) => onAction("buttonClicked", index));
    expect(onSelect.mock.calls).toEqual([
      ["preview"],
      ["browse-files"],
      ["mentioned-files"],
      ["view-diff"],
      ["open-artifact"],
      ["advance-stage"],
      ["close-task"]
    ]);
  });

  it("offers only close for an unresolved task creation on Android", () => {
    nativeMocks.platform.OS = "android";
    const onSelect = vi.fn();

    showTaskActionMenu(
      { mentionedFilesLabel: "Mentioned Files (0)", taskCreation: true },
      onSelect
    );

    const { config, onAction } = androidDialog();
    expect(config.items).toEqual(["Close Task"]);
    onAction("buttonClicked", 0);
    expect(onSelect).toHaveBeenCalledWith("close-task");
  });

  it.each([
    ["Cancel", "buttonClicked", -2],
    ["back or outside tap", "dismissed", undefined]
  ] as const)(
    "routes Android %s through the dismiss callback",
    (_, action, buttonKey) => {
      nativeMocks.platform.OS = "android";
      const onSelect = vi.fn();
      const onDismiss = vi.fn();

      showTaskActionMenu(
        { mentionedFilesLabel: "Mentioned Files (0)" },
        onSelect,
        onDismiss
      );

      androidDialog().onAction(action, buttonKey);
      expect(onSelect).not.toHaveBeenCalled();
      expect(onDismiss).toHaveBeenCalledOnce();
    }
  );

  it("dismisses when Android cannot show the dialog", () => {
    nativeMocks.platform.OS = "android";
    const onDismiss = vi.fn();
    const warn = vi.spyOn(console, "warn").mockImplementation(() => undefined);

    showTaskActionMenu(
      { mentionedFilesLabel: "Mentioned Files (0)" },
      vi.fn(),
      onDismiss
    );

    androidDialog().onError("Tried to show an alert while not attached to an Activity");
    expect(onDismiss).toHaveBeenCalledOnce();
    warn.mockRestore();
  });
});
