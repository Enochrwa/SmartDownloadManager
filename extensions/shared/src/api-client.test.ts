import { describe, expect, it, vi } from "vitest";
import { SdmApiClient, SdmApiError } from "./api-client";

function fakeFetch(
  handler: (url: string, init?: RequestInit) => { status: number; body: unknown },
): typeof fetch {
  return vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    const { status, body } = handler(String(input), init);
    return new Response(JSON.stringify(body), {
      status,
      headers: { "content-type": "application/json" },
    });
  }) as unknown as typeof fetch;
}

describe("SdmApiClient", () => {
  it("sends a bearer token header on authenticated requests", async () => {
    let capturedHeaders: HeadersInit | undefined;
    const fetchImpl = vi.fn(async (_url: RequestInfo | URL, init?: RequestInit) => {
      capturedHeaders = init?.headers;
      return new Response(JSON.stringify([]), { status: 200 });
    }) as unknown as typeof fetch;

    const client = new SdmApiClient("http://127.0.0.1:7890", "secret-token", fetchImpl);
    await client.listJobs();

    expect((capturedHeaders as Record<string, string>).authorization).toBe("Bearer secret-token");
  });

  it("does not send a bearer token for pairing verification", async () => {
    let capturedHeaders: HeadersInit | undefined;
    const fetchImpl = vi.fn(async (_url: RequestInfo | URL, init?: RequestInit) => {
      capturedHeaders = init?.headers;
      return new Response(JSON.stringify({ ok: true }), { status: 200 });
    }) as unknown as typeof fetch;

    const client = new SdmApiClient("http://127.0.0.1:7890", "secret-token", fetchImpl);
    await client.verifyPairingToken("some-token");

    expect((capturedHeaders as Record<string, string>).authorization).toBeUndefined();
  });

  it("converts camelCase request bodies to snake_case on the wire", async () => {
    let capturedBody: string | undefined;
    const fetchImpl = vi.fn(async (_url: RequestInfo | URL, init?: RequestInit) => {
      capturedBody = init?.body as string;
      return new Response(
        JSON.stringify({
          job: {
            id: "job-1",
            url: "https://example.com/a.zip",
            destination: "/tmp/a.zip",
            status: "queued",
            job_kind: "http",
            downloaded_bytes: 0,
            total_bytes: null,
            connections: 1,
            error_class: null,
            error_message: null,
            parent_job_id: null,
          },
          deduplicated: false,
        }),
        { status: 202 },
      );
    }) as unknown as typeof fetch;

    const client = new SdmApiClient("http://127.0.0.1:7890", "tok", fetchImpl);
    const result = await client.capture({
      url: "https://example.com/a.zip",
      pageUrl: "https://example.com/",
      suggestedFilename: null,
      sizeHintBytes: null,
      source: "context-menu",
    });

    const sentBody = JSON.parse(capturedBody ?? "{}");
    expect(sentBody.page_url).toBe("https://example.com/");
    expect(sentBody.size_hint_bytes).toBeNull();

    // ...and snake_case responses come back out as camelCase.
    expect(result.job.jobKind).toBe("http");
    expect(result.job.errorClass).toBeNull();
    expect(result.deduplicated).toBe(false);
  });

  it("throws SdmApiError with the server's error message on failure", async () => {
    const fetchImpl = fakeFetch(() => ({
      status: 401,
      body: { error: "unknown pairing token" },
    }));
    const client = new SdmApiClient("http://127.0.0.1:7890", "bad-token", fetchImpl);

    await expect(client.listJobs()).rejects.toThrow(SdmApiError);
    await expect(client.listJobs()).rejects.toThrow("unknown pairing token");
  });

  it("health() returns false on network failure instead of throwing", async () => {
    const fetchImpl = vi.fn(async () => {
      throw new Error("connection refused");
    }) as unknown as typeof fetch;
    const client = new SdmApiClient("http://127.0.0.1:7890", null, fetchImpl);
    expect(await client.health()).toBe(false);
  });
});
