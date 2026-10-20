import { useEffect, useRef, useState } from "react";
import { invoke, isTauri } from "@tauri-apps/api/core";

type Mode = "off" | "window" | "tray";
type Preference = {
  available: boolean;
  mode: Mode | null;
  message: string | null;
};

export function StartupSettings({ preview }: { preview: boolean }) {
  const [preference, setPreference] = useState<Preference | null>(null);
  const [pending, setPending] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [revision, setRevision] = useState(0);
  const [saved, setSaved] = useState(false);
  const [draft, setDraft] = useState<Mode | null>(null);
  const reading = useRef<{
    revision: number;
    promise: Promise<Preference>;
  } | null>(null);
  const native = isTauri();
  const fixture = import.meta.env.DEV && preview;

  useEffect(() => {
    if (!native && !fixture) return;
    let active = true;
    setPending(true);
    setError(null);
    if (reading.current?.revision !== revision)
      reading.current = {
        revision,
        promise: fixture
          ? Promise.resolve({
              available: true,
              mode: "off" as const,
              message: null,
            })
          : invoke<Preference>("windows_startup"),
      };
    const read = reading.current.promise;
    void read.then(
      (value) => {
        if (active) {
          setPreference(value);
          setPending(false);
        }
      },
      (failure) => {
        if (active) {
          setPreference(null);
          setError(String(failure));
          setPending(false);
        }
      },
    );
    return () => {
      active = false;
    };
  }, [native, fixture, revision]);

  if (!native && !fixture) return null;
  async function change(mode: Mode) {
    if (pending) return;
    setPending(true);
    setDraft(mode);
    setSaved(false);
    setError(null);
    try {
      const value = fixture
        ? { available: true, mode, message: null }
        : await invoke<Preference>("windows_startup", { mode });
      setPreference(value);
      setSaved(value.available);
    } catch (failure) {
      setError(String(failure));
    } finally {
      setDraft(null);
      setPending(false);
    }
  }
  return (
    <section
      aria-labelledby="windows-startup-title"
      className="startup-settings"
      aria-busy={pending}
    >
      <h3 id="windows-startup-title" className="section-label">
        Windows startup
      </h3>
      <p className="field-note">
        When you sign in to Windows, start this interface automatically. DNS
        management stays independent.
      </p>
      {preference?.available && (
        <fieldset className="theme-picker" disabled={pending || !!error}>
          <legend className="sr-only">
            Start Quad9 when signing in to Windows
          </legend>
          {(
            [
              ["off", "Off"],
              ["window", "Open window"],
              ["tray", "Tray"],
            ] as const
          ).map(([mode, label]) => (
            <label key={mode}>
              <input
                type="radio"
                name="windows-startup"
                value={mode}
                checked={(draft ?? preference.mode) === mode}
                onChange={() => void change(mode)}
              />
              <span>{label}</span>
            </label>
          ))}
        </fieldset>
      )}
      {preference && !preference.available && (
        <p className="field-note">{preference.message}</p>
      )}
      {pending && (
        <p className="field-note" aria-live="polite">
          {draft !== null
            ? "Saving Windows startup setting…"
            : "Reading Windows startup setting…"}
        </p>
      )}
      {saved && !pending && (
        <p className="field-note" aria-live="polite">
          Windows startup setting saved.
        </p>
      )}
      {error && (
        <div className="startup-error" role="alert">
          <p>
            Could not read or save Windows startup. {error} Read the setting
            again before retrying.
          </p>
          <button
            onClick={() => {
              setSaved(false);
              setRevision((value) => value + 1);
            }}
            disabled={pending}
          >
            Read startup setting again
          </button>
        </div>
      )}
    </section>
  );
}
