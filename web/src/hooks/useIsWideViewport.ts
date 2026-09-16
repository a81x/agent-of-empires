import { useEffect, useState } from "react";

export function useIsWideViewport(): boolean {
  const [isWide, setIsWide] = useState(
    () => typeof window !== "undefined" && Boolean(window.matchMedia?.("(min-width: 768px)").matches),
  );
  useEffect(() => {
    if (typeof window === "undefined" || !window.matchMedia) return;
    const mql = window.matchMedia("(min-width: 768px)");
    const onChange = () => setIsWide(mql.matches);
    mql.addEventListener?.("change", onChange);
    window.addEventListener("resize", onChange);
    return () => {
      mql.removeEventListener?.("change", onChange);
      window.removeEventListener("resize", onChange);
    };
  }, []);
  return isWide;
}
