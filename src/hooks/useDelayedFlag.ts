import { useEffect, useState } from "react";

/**
 * `flag`, but only once it has stayed true for `delay` ms; false again as soon as `flag` is.
 * For loading cues that should not flash on fast operations.
 */
export function useDelayedFlag(flag: boolean, delay = 400): boolean {
  const [shown, setShown] = useState(false);
  useEffect(() => {
    if (!flag) {
      setShown(false);
      return;
    }
    const timer = setTimeout(() => setShown(true), delay);
    return () => clearTimeout(timer);
  }, [flag, delay]);
  return flag && shown;
}
