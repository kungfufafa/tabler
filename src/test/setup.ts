import "@testing-library/jest-dom/vitest";
import { cleanup, configure } from "@testing-library/react";
import { afterEach } from "vitest";

afterEach(() => cleanup());

// findBy*/waitFor give up after 1 s by default, which a busy (Windows) machine overshoots while a
// dialog mounts or a mock round-trip settles. This is only the ceiling of the polling: passing
// tests do not wait longer.
configure({ asyncUtilTimeout: 5000 });

// Pin the default locale to en-US. The app formats counts and dates with the user's locale on
// purpose, and the tests expect en-US text ("1,000", "Oct 12"), but Node takes its default from
// the OS (on Windows it ignores LANG), so an Indonesian Windows would render "1.000". Only calls
// that leave the locale out are pinned; an explicit locale still wins.
const TEST_LOCALE = "en-US";
const PINNED = Symbol.for("tabler.test.localePinned");
// Once per worker: the globals outlive a test file when workers are reused.
if (!(PINNED in Intl)) {
  Object.defineProperty(Intl, PINNED, { value: true });
  const pin = (locales: Intl.LocalesArgument) => locales ?? TEST_LOCALE;
  const { NumberFormat, DateTimeFormat } = Intl;
  Intl.NumberFormat = class extends NumberFormat {
    constructor(locales?: Intl.LocalesArgument, options?: Intl.NumberFormatOptions) {
      super(pin(locales), options);
    }
  } as typeof Intl.NumberFormat;
  Intl.DateTimeFormat = class extends DateTimeFormat {
    constructor(locales?: Intl.LocalesArgument, options?: Intl.DateTimeFormatOptions) {
      super(pin(locales), options);
    }
  } as typeof Intl.DateTimeFormat;
  for (const proto of [Number.prototype, BigInt.prototype, Date.prototype] as const) {
    for (const name of ["toLocaleString", "toLocaleDateString", "toLocaleTimeString"] as const) {
      const original = (proto as unknown as Record<string, unknown>)[name];
      if (typeof original !== "function") continue;
      Object.defineProperty(proto, name, {
        configurable: true,
        writable: true,
        value(this: unknown, locales?: Intl.LocalesArgument, options?: object) {
          return (original as (l: Intl.LocalesArgument, o?: object) => string).call(
            this,
            pin(locales),
            options,
          );
        },
      });
    }
  }
}

// jsdom lacks these; TanStack Virtual and the grid need them to mount.
if (!("ResizeObserver" in globalThis)) {
  class ResizeObserverStub {
    observe() {}
    unobserve() {}
    disconnect() {}
  }
  (globalThis as unknown as { ResizeObserver: unknown }).ResizeObserver = ResizeObserverStub;
}
if (!Element.prototype.scrollTo) {
  Element.prototype.scrollTo = function scrollTo() {} as Element["scrollTo"];
}
