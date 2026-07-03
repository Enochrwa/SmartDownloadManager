import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { ProgressBar } from "./ProgressBar";

describe("ProgressBar", () => {
  it("sets the aria progress value", () => {
    render(<ProgressBar percent={42} />);
    expect(screen.getByRole("progressbar")).toHaveAttribute("aria-valuenow", "42");
  });

  it("clamps out-of-range percentages for the visual width", () => {
    const { container } = render(<ProgressBar percent={150} />);
    const fill = container.querySelector(".sdm-progress-fill") as HTMLElement;
    expect(fill.style.width).toBe("100%");
  });
});
