import { useCallback, useEffect } from "react";
import { registerAppCommands } from "./commands/appCommands";
import { HistoryPalette } from "./commands/HistoryPalette";
import { useKeymap } from "./commands/keymap";
import { SearchEverywhere, useSearchEverywhere } from "./commands/SearchEverywhere";
import { useDataSources } from "./db/dataSources";
import { DataSourceDialog } from "./dialogs/DataSourceDialog";
import { PasswordPrompt } from "./dialogs/PasswordPrompt";
import { useNativeMenu } from "./menu/nativeMenu";
import { StatusBar } from "./workbench/StatusBar";
import { TitleBar } from "./workbench/TitleBar";
import { Workbench } from "./workbench/Workbench";

export default function App() {
  const openSearch = useCallback(() => useSearchEverywhere.getState().show("all"), []);
  useKeymap(openSearch);
  useNativeMenu();

  useEffect(registerAppCommands, []);
  useEffect(() => {
    void useDataSources.getState().load();
  }, []);

  return (
    <div className="flex h-full flex-col">
      <TitleBar />
      <main className="min-h-0 flex-1">
        <Workbench />
      </main>
      <StatusBar />
      <SearchEverywhere />
      <HistoryPalette />
      <DataSourceDialog />
      <PasswordPrompt />
    </div>
  );
}
