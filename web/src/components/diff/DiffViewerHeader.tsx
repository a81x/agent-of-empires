import type { ReactNode } from "react";
import type { RichDiffFile } from "../../lib/types";
import { useWebSettings } from "../../hooks/useWebSettings";
import { LineCounts } from "./DiffFileRows";

const STATUS: Record<string, [label: string, color: string]> = {
  added: ["Added", "text-status-running"],
  modified: ["Modified", "text-status-waiting"],
  deleted: ["Deleted", "text-status-error"],
  renamed: ["Renamed", "text-accent-600"],
  copied: ["Copied", "text-accent-600"],
  untracked: ["Untracked", "text-text-muted"],
  conflicted: ["Conflicted", "text-status-waiting"],
  unchanged: ["Unchanged", "text-text-muted"],
};

const ACTIVE = "bg-brand-600 text-white";
const IDLE = "text-text-dim hover:text-text-secondary";

function ToggleButton({
  pressed,
  onClick,
  title,
  skin,
  children,
}: {
  pressed: boolean;
  onClick: () => void;
  title: string;
  skin?: string;
  children: ReactNode;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      aria-pressed={pressed}
      title={title}
      className={`px-2 py-0.5 text-[11px] font-mono cursor-pointer transition-colors ${skin ?? (pressed ? ACTIVE : IDLE)}`}
    >
      {children}
    </button>
  );
}

const GROUP = "flex items-center rounded border border-surface-700/40 overflow-hidden";

interface Props {
  file: RichDiffFile;
  onClose?: () => void;
  markdownAvailable: boolean;
  showRendered: boolean;
  findOpen: boolean;
  onToggleFind: () => void;
  isWide: boolean;
  splitActive: boolean;
}

export function DiffViewerHeader({
  file,
  onClose,
  markdownAvailable,
  showRendered,
  findOpen,
  onToggleFind,
  isWide,
  splitActive,
}: Props) {
  const { settings, update } = useWebSettings();
  const [label, color] = STATUS[file.status] ?? [file.status, "text-text-muted"];
  const split = settings.diffViewLayout === "split";
  return (
    <div className="px-3 py-2 border-b border-surface-700/20 flex items-center gap-2 shrink-0 flex-wrap">
      {onClose && (
        <button
          onClick={onClose}
          className="text-text-dim hover:text-text-secondary cursor-pointer transition-colors flex items-center gap-1 text-[11px]"
          title="Back to terminal"
          aria-label="Back to terminal"
        >
          <svg
            width="14"
            height="14"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            strokeWidth="1.75"
            strokeLinecap="round"
            strokeLinejoin="round"
          >
            <path d="M15 18l-6-6 6-6" />
          </svg>
          <span className="hidden sm:inline">Terminal</span>
        </button>
      )}
      <span className={`font-mono text-[11px] font-semibold ${color}`}>{label}</span>
      <span className="font-mono text-[12px] text-text-primary truncate">
        {file.old_path ? `${file.old_path} → ${file.path}` : file.path}
      </span>
      <LineCounts additions={file.additions} deletions={file.deletions} />
      <div className="ml-auto flex items-center gap-2">
        {markdownAvailable && (
          <div className={GROUP}>
            <ToggleButton
              pressed={settings.markdownPreview === "rendered"}
              onClick={() => update({ markdownPreview: "rendered" })}
              title="Rendered Markdown"
            >
              Rendered
            </ToggleButton>
            <ToggleButton
              pressed={settings.markdownPreview === "raw"}
              onClick={() => update({ markdownPreview: "raw" })}
              title="Raw Markdown source"
            >
              Raw
            </ToggleButton>
          </div>
        )}
        {!showRendered && (
          <>
            <button
              type="button"
              onClick={onToggleFind}
              aria-pressed={findOpen}
              title="Find in diff (Cmd/Ctrl+F)"
              aria-label="Find in diff"
              className={`px-2 py-0.5 text-[11px] font-mono rounded cursor-pointer transition-colors ${findOpen ? ACTIVE : IDLE}`}
            >
              Find
            </button>
            <div className={GROUP}>
              <ToggleButton
                pressed={settings.diffViewLayout === "unified"}
                onClick={() => update({ diffViewLayout: "unified" })}
                title="Unified diff"
              >
                Unified
              </ToggleButton>
              <ToggleButton
                pressed={split}
                onClick={() => update({ diffViewLayout: "split" })}
                title={
                  split && !isWide
                    ? "Split selected, but this pane is too narrow; showing unified"
                    : "Side-by-side diff"
                }
                skin={split ? (splitActive ? ACTIVE : "bg-brand-600/40 text-white/80") : IDLE}
              >
                Split
              </ToggleButton>
            </div>
          </>
        )}
      </div>
    </div>
  );
}
