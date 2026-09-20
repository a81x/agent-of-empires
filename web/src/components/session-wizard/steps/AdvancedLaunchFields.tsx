const TEXT_INPUT =
  "w-full bg-surface-900 border border-surface-700 rounded-lg px-3 py-2.5 text-sm font-mono text-text-primary placeholder:text-text-dim focus:border-brand-600 focus:outline-none";

function LabeledInput({
  label,
  value,
  onChange,
  placeholder,
  children,
}: {
  label: string;
  value: string;
  onChange: (v: string) => void;
  placeholder: string;
  /** Hint rendered under the input. */
  children?: React.ReactNode;
}) {
  return (
    <div>
      <label className="block text-sm text-text-dim mb-1.5">{label}</label>
      <input
        type="text"
        value={value}
        onChange={(e) => onChange(e.target.value)}
        placeholder={placeholder}
        className={TEXT_INPUT}
      />
      {children}
    </div>
  );
}

function EnvVarList({ values, onChange }: { values: string[]; onChange: (v: string[]) => void }) {
  return (
    <div>
      <label className="block text-sm text-text-dim mb-1.5">Environment variables</label>
      {values.map((env, i) => (
        <div key={i} className="flex gap-2 mb-1">
          <input
            type="text"
            value={env}
            onChange={(e) => onChange(values.map((v, j) => (j === i ? e.target.value : v)))}
            placeholder="KEY=value"
            className="flex-1 bg-surface-900 border border-surface-700 rounded-md px-2 py-1.5 text-sm font-mono text-text-primary placeholder:text-text-dim focus:border-brand-600 focus:outline-none"
          />
          <button
            onClick={() => onChange(values.filter((_, j) => j !== i))}
            className="px-2 text-text-dim hover:text-status-error cursor-pointer"
          >
            &times;
          </button>
        </div>
      ))}
      <button
        onClick={() => onChange([...values, ""])}
        className="text-xs text-text-dim hover:text-text-secondary cursor-pointer"
      >
        + Add variable
      </button>
    </div>
  );
}

interface Props {
  sandboxEnabled: boolean;
  sandboxImage: string;
  extraEnv: string[];
  customInstruction: string;
  extraArgs: string;
  commandOverride: string;
  /** Set when structured view makes `extraArgs` inert. */
  extraArgsIgnored: boolean;
  resolvedCommand: string;
  onChange: (field: string, value: unknown) => void;
}

/** Container, instruction and launch-command knobs behind the wizard's advanced fold. */
export function AdvancedLaunchFields({
  sandboxEnabled,
  sandboxImage,
  extraEnv,
  customInstruction,
  extraArgs,
  commandOverride,
  extraArgsIgnored,
  resolvedCommand,
  onChange,
}: Props) {
  return (
    <div className="space-y-4">
      {sandboxEnabled && (
        <>
          <LabeledInput
            label="Container image"
            value={sandboxImage}
            onChange={(v) => onChange("sandboxImage", v)}
            placeholder="ghcr.io/agent-of-empires/aoe-sandbox:latest"
          />
          <EnvVarList values={extraEnv} onChange={(v) => onChange("extraEnv", v)} />
        </>
      )}

      <div>
        <label className="block text-sm text-text-dim mb-1.5">Agent instructions</label>
        <textarea
          value={customInstruction}
          onChange={(e) => onChange("customInstruction", e.target.value)}
          placeholder="Custom instructions for this session..."
          rows={3}
          className="w-full bg-surface-900 border border-surface-700 rounded-lg px-3 py-2 text-sm text-text-primary placeholder:text-text-dim focus:border-brand-600 focus:outline-none resize-y"
        />
      </div>

      <LabeledInput
        label="Additional arguments"
        value={extraArgs}
        onChange={(v) => onChange("extraArgs", v)}
        placeholder="e.g. --port 8080"
      >
        {extraArgsIgnored && (
          <p className="mt-1.5 text-xs text-status-warning" data-testid="extra-args-ignored">
            Extra args are ignored for structured-view sessions; use the command override to change the launch command.
          </p>
        )}
      </LabeledInput>

      <LabeledInput
        label="Command override"
        value={commandOverride}
        onChange={(v) => onChange("commandOverride", v)}
        placeholder="Override the agent launch command"
      >
        {resolvedCommand && (
          <p className="mt-1.5 text-xs text-text-dim" data-testid="resolved-launch-command">
            Resolved launch command: <code className="font-mono text-text-secondary">{resolvedCommand}</code>
          </p>
        )}
      </LabeledInput>
    </div>
  );
}
