import { useState } from "react";
import { EngineIcon } from "../ui/EngineIcon";
import { inputClass, Modal } from "../ui/Modal";
import { Button } from "../ui/primitives";
import { useDialogs } from "./dialogs";

/** Asks for the password of a data source that does not save it. */
export function PasswordPrompt() {
  const request = useDialogs((s) => s.password);
  const [password, setPassword] = useState("");
  if (!request) return null;

  const { source, resolve } = request;
  const finish = (value: string | null) => {
    setPassword("");
    resolve(value);
  };
  const target = `${source.params.user || "(no user)"}@${source.params.host || "localhost"}`;

  return (
    <Modal
      open
      onClose={() => finish(null)}
      title={`Connect to ${source.name}`}
      description={
        <span className="flex items-center gap-1.5">
          <EngineIcon engine={source.params.engine} /> {target}
        </span>
      }
      width={400}
      footer={
        <div className="ml-auto flex gap-2">
          <Button variant="ghost" onClick={() => finish(null)}>
            Cancel
          </Button>
          <Button variant="primary" onClick={() => finish(password)}>
            Connect
          </Button>
        </div>
      }
    >
      <form
        onSubmit={(e) => {
          e.preventDefault();
          finish(password);
        }}
      >
        <input
          autoFocus
          type="password"
          aria-label="Password"
          placeholder="Password"
          className={inputClass}
          value={password}
          onChange={(e) => setPassword(e.target.value)}
        />
        <p className="mt-2 text-[11.5px] text-subtle">Remembered until IdeDB quits.</p>
      </form>
    </Modal>
  );
}
