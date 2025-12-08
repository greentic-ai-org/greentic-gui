export type InitOptions = {
  tenantDomain?: string;
  configUrl?: string;
  eventsUrl?: string;
  workerMessageUrl?: string;
};

export type AttachWorkerOptions = {
  workerId: string;
  selector: string;
  routes?: string[];
};

export type SendWorkerMessageOptions = {
  workerId: string;
  payload: unknown;
  context?: Record<string, unknown>;
};

export type SendEventOptions = {
  eventType: string;
  metadata?: Record<string, unknown>;
};

export type StartSessionOptions = {
  userId: string;
  team?: string;
};

export interface GreenticGUIApi {
  version: string;
  init(options?: InitOptions): Promise<void>;
  attachWorker(options: AttachWorkerOptions): void;
  sendWorkerMessage(options: SendWorkerMessageOptions): Promise<unknown>;
  sendEvent(options: SendEventOptions): Promise<void>;
  startSession?(options: StartSessionOptions): Promise<unknown>;
}

declare global {
  interface Window {
    GreenticGUI: GreenticGUIApi;
  }
}
