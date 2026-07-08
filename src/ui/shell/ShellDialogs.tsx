import { useShellStore } from "../../stores/shellStore";
import { NewProjectWizard } from "./NewProjectWizard";

/** The project-entry overlays (currently just the New Project wizard), rendered
 *  once via the always-mounted MenuBar so both the Home screen and the File menu
 *  can open them. */
export function ShellDialogs() {
  const wizardOpen = useShellStore((s) => s.wizardOpen);
  const wizardInitialFolder = useShellStore((s) => s.wizardInitialFolder);
  const closeWizard = useShellStore((s) => s.closeWizard);

  return (
    <>
      {wizardOpen && (
        <NewProjectWizard onClose={closeWizard} initialFolder={wizardInitialFolder} />
      )}
    </>
  );
}
