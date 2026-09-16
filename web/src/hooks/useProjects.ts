import { useCallback, useEffect, useState } from "react";
import { fetchProjects } from "../lib/api";
import type { ProjectInfo } from "../lib/types";

export function useProjects(): {
  projects: ProjectInfo[];
  refresh: () => Promise<void>;
} {
  const [projects, setProjects] = useState<ProjectInfo[]>([]);

  const refresh = useCallback(async () => {
    setProjects(await fetchProjects());
  }, []);

  useEffect(() => {
    void fetchProjects().then(setProjects);
    const onFocus = () => {
      if (document.visibilityState === "visible") void fetchProjects().then(setProjects);
    };
    window.addEventListener("focus", onFocus);
    document.addEventListener("visibilitychange", onFocus);
    return () => {
      window.removeEventListener("focus", onFocus);
      document.removeEventListener("visibilitychange", onFocus);
    };
  }, [refresh]);

  return { projects, refresh };
}
