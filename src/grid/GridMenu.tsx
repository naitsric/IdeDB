import { CommandItem, ContextMenuContent, ContextMenuSeparator } from "../ui/ContextMenu";

/** Right-click menu of a result grid; every item is a command, so shortcuts and enablement match. */
export function GridMenu({ isTable, editable }: { isTable: boolean; editable: boolean }) {
  return (
    <ContextMenuContent>
      <CommandItem id="grid.copy" />
      <CommandItem id="grid.copyCsv" />
      <CommandItem id="grid.copyJson" />
      <CommandItem id="grid.copyInserts" />
      {editable && (
        <>
          <ContextMenuSeparator />
          <CommandItem id="grid.setNull" />
          <CommandItem id="grid.addRow" />
          <CommandItem id="grid.duplicateRows" />
          <CommandItem id="grid.deleteRows" />
          <CommandItem id="grid.revertSelected" />
          <CommandItem id="grid.submit" />
        </>
      )}
      {isTable && (
        <>
          <ContextMenuSeparator />
          <CommandItem id="grid.goToReferenced" />
        </>
      )}
      <ContextMenuSeparator />
      <CommandItem id="grid.exportCsv" />
      <CommandItem id="grid.exportJson" />
      <CommandItem id="grid.exportSql" />
      <ContextMenuSeparator />
      <CommandItem id="grid.valueViewer" />
    </ContextMenuContent>
  );
}
