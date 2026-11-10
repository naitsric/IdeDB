import { useState } from "react";
import { EngineIcon } from "../ui/EngineIcon";
import { useT } from "../i18n";
import { inputClass, Modal } from "../ui/Modal";
import { Button } from "../ui/primitives";
import { useDialogs } from "./dialogs";

/** Asks for the password of a data source that does not save it. */
export function PasswordPrompt() {
  const t = useT();
  const request = useDialogs((s) => s.password);
  const [password, setPassword] = useState("");
  if (!request) return null;

  const { source, resolve } = request;
  const finish = (value: string | null) => {
    setPassword("");
    resolve(value);
  };
  const target = `${source.params.user || t("password.noUser")}@${source.params.host || "localhost"}`;

  return (
    <Modal
      open
      onClose={() => finish(null)}
      title={t("password.title", { name: source.name })}
      description={
        <span className="flex items-center gap-1.5">
          <EngineIcon engine={source.params.engine} /> {target}
        </span>
      }
      width={400}
      footer={
        <div className="ml-auto flex gap-2">
          <Button variant="ghost" onClick={() => finish(null)}>
            {t("common.cancel")}
          </Button>
          <Button variant="primary" onClick={() => finish(password)}>
            {t("command.datasource.connect")}
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
          aria-label={t("datasource.dialog.password")}
          placeholder={t("datasource.dialog.password")}
          className={inputClass}
          value={password}
          onChange={(e) => setPassword(e.target.value)}
        />
        <p className="mt-2 text-[11.5px] text-subtle">{t("password.remembered")}</p>
      </form>
    </Modal>
  );
}
