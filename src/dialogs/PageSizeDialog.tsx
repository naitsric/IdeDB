import { useEffect, useState } from "react";
import { PAGE_SIZE_CHOICES, parsePageSize, setPageSize, useFetchSettings } from "../db/fetching";
import { inputClass, Modal } from "../ui/Modal";
import { Button, cx } from "../ui/primitives";

const count = new Intl.NumberFormat("en-US");

/** How many rows a statement loads before pausing (Result Page Size…). */
export function PageSizeDialog() {
  const { pageSize, dialogOpen } = useFetchSettings();
  const [text, setText] = useState(String(pageSize));
  useEffect(() => {
    if (dialogOpen) setText(String(pageSize));
  }, [dialogOpen, pageSize]);

  const parsed = parsePageSize(text);
  const close = () => useFetchSettings.setState({ dialogOpen: false });
  const save = (value: number) => {
    setPageSize(value);
    close();
  };

  return (
    <Modal
      open={dialogOpen}
      onClose={close}
      title="Result Page Size"
      description="Rows a statement loads before pausing. Scrolling to the end or Fetch All loads the rest."
      width={420}
      footer={
        <div className="ml-auto flex gap-2">
          <Button variant="ghost" onClick={close}>
            Cancel
          </Button>
          <Button variant="primary" disabled={parsed === null} onClick={() => parsed !== null && save(parsed)}>
            Save
          </Button>
        </div>
      }
    >
      <form
        className="flex flex-col gap-3"
        onSubmit={(e) => {
          e.preventDefault();
          if (parsed !== null) save(parsed);
        }}
      >
        <div className="flex gap-1.5">
          {PAGE_SIZE_CHOICES.map((choice) => (
            <button
              key={choice}
              type="button"
              onClick={() => setText(String(choice))}
              className={cx(
                "h-7 rounded-md border px-2.5 text-[12px] tabular-nums transition-colors",
                parsed === choice
                  ? "border-accent bg-accent-soft text-fg"
                  : "border-border text-muted hover:border-border-strong hover:text-fg",
              )}
            >
              {count.format(choice)}
            </button>
          ))}
        </div>
        <input
          autoFocus
          inputMode="numeric"
          aria-label="Rows per page"
          className={cx(inputClass, parsed === null && "border-danger focus:border-danger")}
          value={text}
          onChange={(e) => setText(e.target.value)}
        />
        {parsed === null && <p className="text-[11.5px] text-danger">A whole number from 1 to 1,000,000.</p>}
      </form>
    </Modal>
  );
}
