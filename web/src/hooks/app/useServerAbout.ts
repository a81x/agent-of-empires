// Server identity (/api/about) and the telemetry consent state that loads alongside it.

import { useCallback, useEffect, useState } from "react";
import {
  fetchAbout,
  fetchTelemetryStatus,
  reportTelemetrySeen,
  setTelemetryConsent,
  type ServerAbout,
} from "../../lib/api";

export function useServerAbout() {
  const [serverAbout, setServerAbout] = useState<ServerAbout | null>(null);
  const [serverAboutLoaded, setServerAboutLoaded] = useState(false);
  const [telemetryConsentNeeded, setTelemetryConsentNeeded] = useState(false);
  const [telemetryConsentKnown, setTelemetryConsentKnown] = useState(false);

  const refreshServerAbout = useCallback(async () => {
    try {
      const about = await fetchAbout();
      if (about) setServerAbout(about);
    } finally {
      setServerAboutLoaded(true);
    }
  }, []);

  useEffect(() => {
    let active = true;
    void fetchAbout()
      .then((about) => {
        if (!active) return;
        if (about) setServerAbout(about);
        if (about && !about.read_only) reportTelemetrySeen("web");
      })
      .finally(() => {
        if (active) setServerAboutLoaded(true);
      });
    void fetchTelemetryStatus()
      .then((status) => {
        if (active && status && !status.responded && !status.do_not_track) setTelemetryConsentNeeded(true);
      })
      .finally(() => {
        if (active) setTelemetryConsentKnown(true);
      });
    return () => {
      active = false;
    };
  }, []);

  const chooseTelemetryConsent = useCallback((enabled: boolean) => {
    setTelemetryConsentNeeded(false);
    void setTelemetryConsent(enabled);
  }, []);

  return {
    serverAbout,
    serverAboutLoaded,
    refreshServerAbout,
    telemetryConsentNeeded,
    telemetryConsentKnown,
    chooseTelemetryConsent,
  };
}
