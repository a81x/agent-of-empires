import { useWebSettings } from "../../hooks/useWebSettings";

/// Per-browser pane auto-open defaults; they apply to sessions opened afterwards.
export function PanelsSettings() {
  const { settings, update } = useWebSettings();

  return (
    <div>
      <div className="space-y-4">
        <div>
          <label className="flex items-center justify-between gap-3 cursor-pointer">
            <div>
              <div className="text-[13px] text-text-secondary">Open the diff panel in new sessions</div>
              <p className="text-[11px] text-text-muted mt-1">
                Show the diff pane automatically when a session first opens. Turn off to start with it closed; open it
                any time from the activity bar.
              </p>
            </div>
            <input
              type="checkbox"
              checked={settings.autoOpenDiffPane}
              onChange={(e) => update({ autoOpenDiffPane: e.target.checked })}
              className="accent-brand-600 w-4 h-4 shrink-0"
            />
          </label>
        </div>

        <div>
          <label className="flex items-center justify-between gap-3 cursor-pointer">
            <div>
              <div className="text-[13px] text-text-secondary">Open a terminal panel in new sessions</div>
              <p className="text-[11px] text-text-muted mt-1">
                Show a terminal pane automatically when a session first opens. Turn off to start with it closed.
              </p>
            </div>
            <input
              type="checkbox"
              checked={settings.autoOpenTerminalPane}
              onChange={(e) => update({ autoOpenTerminalPane: e.target.checked })}
              className="accent-brand-600 w-4 h-4 shrink-0"
            />
          </label>
        </div>

        <div>
          <label className="flex items-center justify-between gap-3 cursor-pointer">
            <div>
              <div className="text-[13px] text-text-secondary">Open plugin panels automatically</div>
              <p className="text-[11px] text-text-muted mt-1">
                Auto-open panes contributed by installed plugins, such as the GitHub pull-request pane. Turn off to keep
                them closed until you open one from the activity bar.
              </p>
            </div>
            <input
              type="checkbox"
              checked={settings.autoOpenPluginPanes}
              onChange={(e) => update({ autoOpenPluginPanes: e.target.checked })}
              className="accent-brand-600 w-4 h-4 shrink-0"
            />
          </label>
        </div>
      </div>
    </div>
  );
}
