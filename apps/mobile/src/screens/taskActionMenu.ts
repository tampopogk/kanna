import {
  ActionSheetIOS,
  Platform,
  TurboModuleRegistry,
  type TurboModule
} from "react-native";
import type { TaskStageAction } from "../state/sessionStore";

export type TaskAction =
  | "preview"
  | "browse-files"
  | "mentioned-files"
  | "view-diff"
  | "open-artifact"
  | TaskStageAction;

interface TaskActionDefinition {
  id: TaskAction;
  label: string;
  style?: "destructive";
}

export interface TaskActionMenuOptions {
  mentionedFilesLabel: string;
  taskCreation?: boolean;
  previewAvailable?: boolean;
  artifactsAvailable?: boolean;
}

const MENU_TITLE = "Task Actions";
const CANCEL_LABEL = "Cancel";

/**
 * React Native's Android dialog module, which `Alert.alert` wraps. `Alert`
 * keeps only three buttons (positive, negative, neutral) and drops the rest,
 * so a longer menu goes through the module's `items` list instead: an
 * AlertDialog item list with no cap. A tapped item reports `buttonClicked`
 * with its index (>= 0); the buttons report negative keys.
 */
interface AndroidDialogManager extends TurboModule {
  getConstants(): { buttonClicked: string; dismissed: string };
  showAlert(
    config: {
      title?: string;
      items?: string[];
      buttonNegative?: string;
      cancelable?: boolean;
    },
    onError: (error: string) => void,
    onAction: (action: string, buttonKey?: number) => void
  ): void;
}

export function showTaskActionMenu(
  options: TaskActionMenuOptions,
  onSelect: (action: TaskAction) => void,
  onDismiss: () => void = () => undefined
): void {
  const allTaskActions: readonly TaskActionDefinition[] = [
    ...(options.previewAvailable
      ? [{ id: "preview" as const, label: "Preview Dev Server" }]
      : []),
    { id: "browse-files", label: "Browse Files" },
    { id: "mentioned-files", label: options.mentionedFilesLabel },
    { id: "view-diff", label: "View Diff" },
    ...(options.artifactsAvailable
      ? [{ id: "open-artifact" as const, label: "Open Artifact…" }]
      : []),
    { id: "advance-stage", label: "Advance Stage" },
    { id: "close-task", label: "Close Task", style: "destructive" }
  ];
  const taskActions = options.taskCreation
    ? allTaskActions.filter((action) => action.id === "close-task")
    : allTaskActions;
  if (Platform.OS === "ios") {
    ActionSheetIOS.showActionSheetWithOptions(
      {
        title: MENU_TITLE,
        options: [...taskActions.map((action) => action.label), CANCEL_LABEL],
        cancelButtonIndex: taskActions.length,
        destructiveButtonIndex: taskActions.findIndex(
          (action) => action.style === "destructive"
        )
      },
      (buttonIndex) => {
        const action = taskActions[buttonIndex];
        if (action) {
          onSelect(action.id);
        } else {
          onDismiss();
        }
      }
    );
    return;
  }

  const dialogManager =
    TurboModuleRegistry.get<AndroidDialogManager>("DialogManagerAndroid");
  if (!dialogManager) {
    onDismiss();
    return;
  }
  const { buttonClicked } = dialogManager.getConstants();
  dialogManager.showAlert(
    {
      title: MENU_TITLE,
      items: taskActions.map((action) => action.label),
      buttonNegative: CANCEL_LABEL,
      cancelable: true
    },
    (error) => {
      console.warn(error);
      onDismiss();
    },
    (dialogAction, buttonKey) => {
      const action =
        dialogAction === buttonClicked && buttonKey !== undefined
          ? taskActions[buttonKey]
          : undefined;
      if (action) {
        onSelect(action.id);
      } else {
        onDismiss();
      }
    }
  );
}
