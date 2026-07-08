import {
  type BatchCaptureRequest,
  type BatchCaptureResponse,
  type CaptureRequest,
  type CaptureResponse,
  type JobResponse,
  type PairingStatusResponse,
  type PairingTokenIssueResponse,
  type PairingVerifyResponse,
  camelToSnake,
  snakeToCamel,
} from "./api-types";

export class SdmApiError extends Error {
  constructor(
    message: string,
    public readonly status: number,
  ) {
    super(message);
    this.name = "SdmApiError";
  }
}

/** Injected instead of imported directly so tests can supply a fake without touching global state. */
export type FetchLike = typeof fetch;

/**
 * Thin REST client the extension uses exclusively to talk to `sdmd` —
 * per Sprint 11's scope note, the extension never touches the engine or
 * filesystem directly. Every method takes `baseUrl`/`token` explicitly
 * rather than reading extension storage itself, so it stays testable
 * with a fake `fetch` and no `chrome.*` mocking at all.
 */
export class SdmApiClient {
  constructor(
    private readonly baseUrl: string,
    private readonly token: string | null,
    private readonly fetchImpl: FetchLike = fetch,
  ) {}

  private async request<T>(
    method: string,
    path: string,
    body?: unknown,
    authenticated = true,
  ): Promise<T> {
    const headers: Record<string, string> = {};
    if (body !== undefined) headers["content-type"] = "application/json";
    if (authenticated && this.token) headers.authorization = `Bearer ${this.token}`;

    const response = await this.fetchImpl(`${this.baseUrl}${path}`, {
      method,
      headers,
      body: body !== undefined ? JSON.stringify(camelToSnake(body)) : undefined,
    });

    const text = await response.text();
    const parsed = text.length > 0 ? JSON.parse(text) : {};
    if (!response.ok) {
      const message =
        typeof parsed === "object" && parsed !== null && "error" in parsed
          ? String((parsed as { error: unknown }).error)
          : `request failed with status ${response.status}`;
      throw new SdmApiError(message, response.status);
    }
    return snakeToCamel(parsed) as T;
  }

  async health(): Promise<boolean> {
    try {
      const res = await this.fetchImpl(`${this.baseUrl}/health`);
      return res.ok;
    } catch {
      return false;
    }
  }

  async issuePairingToken(label?: string): Promise<PairingTokenIssueResponse> {
    return this.request("POST", "/pairing/tokens", { label }, false);
  }

  async verifyPairingToken(token: string): Promise<PairingVerifyResponse> {
    return this.request("POST", "/pairing/verify", { token }, false);
  }

  async pairingStatus(): Promise<PairingStatusResponse> {
    return this.request("GET", "/pairing/status", undefined, false);
  }

  async listJobs(): Promise<JobResponse[]> {
    return this.request("GET", "/jobs");
  }

  async capture(req: CaptureRequest): Promise<CaptureResponse> {
    return this.request("POST", "/capture", req);
  }

  async captureBatch(req: BatchCaptureRequest): Promise<BatchCaptureResponse> {
    return this.request("POST", "/capture/batch", req);
  }
}
