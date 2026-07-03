import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { StatusBadge } from "./StatusBadge";

describe("StatusBadge", () => {
  it("renders a human-readable label for each status", () => {
    render(<StatusBadge status="downloading" />);
    expect(screen.getByText("Downloading")).toBeInTheDocument();
  });

  it("renders the completed label", () => {
    render(<StatusBadge status="completed" />);
    expect(screen.getByText("Completed")).toBeInTheDocument();
  });

  it("exposes the raw status as a data attribute", () => {
    render(<StatusBadge status="failed" />);
    expect(screen.getByText("Failed")).toHaveAttribute("data-status", "failed");
  });
});
