import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createJobToast } from "./jobToast";

const toast = vi.hoisted(() => ({ loading: vi.fn(), dismiss: vi.fn() }));
vi.mock("sonner", () => ({ toast }));

describe("createJobToast", () => {
  let t = 0;
  const now = () => t;
  beforeEach(() => {
    vi.useFakeTimers();
    t = 1_000;
  });
  afterEach(() => {
    vi.useRealTimers();
    toast.loading.mockReset();
    toast.dismiss.mockReset();
  });

  it("shows a lasting toast with Show, throttles updates and dismisses at the end", () => {
    const onShow = vi.fn();
    const job = createJobToast({ id: "job", onShow, throttleMs: 500, now });
    job.sync(null);
    expect(toast.loading).not.toHaveBeenCalled();
    expect(toast.dismiss).not.toHaveBeenCalled();

    job.sync({ title: "Exporting users.csv", description: "1 row" });
    expect(toast.loading).toHaveBeenCalledTimes(1);
    const [title, opts] = toast.loading.mock.lastCall!;
    expect(title).toBe("Exporting users.csv");
    expect(opts).toMatchObject({
      id: "job",
      description: "1 row",
      duration: Number.POSITIVE_INFINITY,
      action: { label: "Show" },
    });
    opts.action.onClick();
    expect(onShow).toHaveBeenCalled();

    // Within the throttle window only the latest text goes out, once, when it ends.
    t += 100;
    job.sync({ title: "Exporting users.csv", description: "2 rows" });
    job.sync({ title: "Exporting users.csv", description: "3 rows" });
    expect(toast.loading).toHaveBeenCalledTimes(1);
    t += 400;
    vi.advanceTimersByTime(400);
    expect(toast.loading).toHaveBeenCalledTimes(2);
    expect(toast.loading.mock.lastCall![1].description).toBe("3 rows");

    // The same text again is no update.
    t += 1_000;
    job.sync({ title: "Exporting users.csv", description: "3 rows" });
    expect(toast.loading).toHaveBeenCalledTimes(2);

    job.sync(null);
    expect(toast.dismiss).toHaveBeenCalledWith("job");
    vi.advanceTimersByTime(1_000);
    expect(toast.loading).toHaveBeenCalledTimes(2);
  });

  it("stays closed once the user dismissed it, until the window opened again", () => {
    const job = createJobToast({ id: "job", onShow: () => undefined, now });
    job.sync({ title: "Backing up db", description: "1 KB written" });
    toast.loading.mock.lastCall![1].onDismiss();
    t += 1_000;
    job.sync({ title: "Backing up db", description: "2 KB written" });
    expect(toast.loading).toHaveBeenCalledTimes(1);
    // The window opened (no toast) and closed again: the toast is back.
    job.sync(null);
    job.sync({ title: "Backing up db", description: "3 KB written" });
    expect(toast.loading).toHaveBeenCalledTimes(2);
  });
});
