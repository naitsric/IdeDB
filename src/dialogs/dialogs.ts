import { create } from "zustand";
import type { DataSource } from "../db/api";

interface DialogsState {
  /** `null` = closed; a data source without id = new. */
  dataSource: DataSource | null;
  password: { source: DataSource; resolve: (password: string | null) => void } | null;
  openDataSource: (source: DataSource) => void;
  closeDataSource: () => void;
}

export const useDialogs = create<DialogsState>((set) => ({
  dataSource: null,
  password: null,
  openDataSource: (source) => set({ dataSource: source }),
  closeDataSource: () => set({ dataSource: null }),
}));

/** Asks for a data source's password; resolves to `null` if dismissed. */
export function requestPassword(source: DataSource): Promise<string | null> {
  return new Promise((resolve) => {
    useDialogs.getState().password?.resolve(null);
    useDialogs.setState({
      password: {
        source,
        resolve: (password) => {
          useDialogs.setState({ password: null });
          resolve(password);
        },
      },
    });
  });
}

export function newDataSource(): DataSource {
  return {
    id: "",
    name: "",
    params: { engine: "postgres", host: "localhost", port: null, user: "", database: "", sslMode: "prefer", path: "" },
    color: null,
    savePassword: true,
  };
}
