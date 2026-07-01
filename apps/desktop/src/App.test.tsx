import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { App } from "./App";

describe("App", () => {
  it("renders the app title", () => {
    render(<App />);
    expect(screen.getByText("SmartDownloadManager")).toBeInTheDocument();
  });

  it("renders the URL input", () => {
    render(<App />);
    expect(screen.getByPlaceholderText("Paste a URL to download")).toBeInTheDocument();
  });
});
