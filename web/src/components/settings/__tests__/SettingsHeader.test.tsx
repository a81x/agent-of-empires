// @vitest-environment jsdom

import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";

vi.mock("../../../lib/api", () => ({
  fetchProfiles: vi.fn().mockResolvedValue([{ name: "default", is_default: true }]),
  createProfile: vi.fn(),
  renameProfile: vi.fn(),
  deleteProfile: vi.fn(),
}));

import { SettingsHeader } from "../SettingsHeader";

afterEach(() => {
  cleanup();
});

describe("SettingsHeader", () => {
  const baseProps = {
    onClose: () => {},
    saving: false,
    saveError: null as string | null,
    selectedProfile: "default",
    onSelectProfile: () => {},
    schema: [],
    schemaLoading: false,
    onSearchJump: () => {},
  };

  it("renders Back button and Settings title", () => {
    render(<SettingsHeader {...baseProps} />);
    expect(screen.getByRole("button", { name: /Back/ })).toBeTruthy();
    expect(screen.getByText("Settings")).toBeTruthy();
  });

  it("does not render Saving... indicator when saving is false", () => {
    render(<SettingsHeader {...baseProps} saving={false} />);
    expect(screen.queryByText("Saving...")).toBeNull();
  });

  it("renders Saving... indicator when saving is true", () => {
    render(<SettingsHeader {...baseProps} saving={true} />);
    expect(screen.getByText("Saving...")).toBeTruthy();
  });

  it("does not render saveError span when saveError is null", () => {
    render(<SettingsHeader {...baseProps} saveError={null} />);
    expect(screen.queryByTestId("settings-header-save-error")).toBeNull();
  });

  it("renders saveError message when saveError is set", () => {
    render(<SettingsHeader {...baseProps} saveError="Save failed: network error" />);
    const errorSpan = screen.getByTestId("settings-header-save-error");
    expect(errorSpan.textContent).toBe("Save failed: network error");
  });

  it("renders both Saving... and saveError together when both are set", () => {
    // The header allows saveError to surface while a subsequent save is in
    // flight; both branches should render side by side.
    render(<SettingsHeader {...baseProps} saving={true} saveError="Save failed: network error" />);
    expect(screen.getByText("Saving...")).toBeTruthy();
    expect(screen.getByText("Save failed: network error")).toBeTruthy();
  });

  it("calls onClose when the Back button is clicked", () => {
    const onClose = vi.fn();
    render(<SettingsHeader {...baseProps} onClose={onClose} />);
    fireEvent.click(screen.getByRole("button", { name: /Back/ }));
    expect(onClose).toHaveBeenCalledTimes(1);
  });
});
