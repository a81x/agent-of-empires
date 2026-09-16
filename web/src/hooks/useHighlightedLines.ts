import { useEffect, useRef, useState } from "react";
import { getSnippetHighlighter, langHintForPath, type ThemedToken } from "../lib/snippetHighlighter";
import type { RichDiffHunk } from "../lib/types";
import { useShikiTheme } from "./useShikiTheme";

export interface SyntaxToken {
  content: string;
  color?: string;
}

export type TokenGrid = SyntaxToken[][][];

interface GridState {
  grid: TokenGrid;
  path: string;
}

export interface HighlightResult {
  tokens: TokenGrid | null;
}

export function useHighlightedLines(hunks: RichDiffHunk[], filePath: string): HighlightResult {
  const [state, setState] = useState<GridState | null>(null);
  const requestRef = useRef(0);
  const isMountedRef = useRef(true);
  const shiki = useShikiTheme();

  useEffect(() => {
    isMountedRef.current = true;
    return () => {
      isMountedRef.current = false;
    };
  }, []);

  useEffect(() => {
    const reqId = ++requestRef.current;

    const langHint = langHintForPath(filePath);
    if (!langHint) return;

    (async () => {
      try {
        const resolved = await getSnippetHighlighter({ langHint, theme: shiki.theme, appearance: shiki.appearance });

        if (!isMountedRef.current || reqId !== requestRef.current) return;

        if (!resolved) {
          setState({ grid: [], path: filePath });
          return;
        }
        const { highlighter: hl, langId, theme: resolvedTheme } = resolved;

        const result: TokenGrid = [];

        for (const hunk of hunks) {
          const hunkTokens: SyntaxToken[][] = [];
          for (const line of hunk.lines) {
            const raw = line.content.replace(/\r?\n$/, "");
            if (!raw) {
              hunkTokens.push([]);
              continue;
            }
            try {
              const { tokens } = hl.codeToTokens(raw, {
                lang: langId,
                theme: resolvedTheme,
              });
              const mapped: SyntaxToken[] = (tokens[0] as ThemedToken[] | undefined)?.map((t) => ({
                content: t.content,
                color: t.color,
              })) ?? [{ content: raw }];
              hunkTokens.push(mapped);
            } catch {
              hunkTokens.push([{ content: raw }]);
            }
          }
          result.push(hunkTokens);
        }

        if (isMountedRef.current && reqId === requestRef.current) {
          setState({ grid: result, path: filePath });
        }
      } catch (err) {
        if (isMountedRef.current && reqId === requestRef.current) {
          console.error("useHighlightedLines: highlighter failed", err);
          setState({ grid: [], path: filePath });
        }
      }
    })();
  }, [hunks, filePath, shiki.theme, shiki.appearance]);

  const tokens = state && state.path === filePath ? state.grid : null;
  return { tokens };
}
