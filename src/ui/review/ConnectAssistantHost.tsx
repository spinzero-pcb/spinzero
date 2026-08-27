import { useRunLauncherStore } from "../../stores/runLauncherStore";
import { ConnectAssistant } from "./ConnectAssistant";

/** Mounts the connect screen from the launcher store, so the dialog itself stays a
 *  plain component that takes `onClose` and can be rendered from a test without the
 *  store. Same split `ShellDialogs` uses for the project wizard. */
export function ConnectAssistantHost() {
  const open = useRunLauncherStore((s) => s.connectOpen);
  const close = useRunLauncherStore((s) => s.closeConnect);
  return open ? <ConnectAssistant onClose={close} /> : null;
}
