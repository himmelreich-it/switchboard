// Stand-ins for server.ts, runner.ts, agents.ts so apptools.ts loads alone. Only what registration needs works.
export type McpServer = any;
export const HOST = "127.0.0.1";
export const PORT = 4477;
const TOPICS = [{ id: 2, name: "Alpha" }, { id: 1, name: "beta" }];
const TODOS: Record<number, any> = { 1: { id: 1, agent: null, run_started_at: null }, 2: { id: 2, agent: "codex", run_started_at: null } };
export const db = {
  query: (sql: string) => ({
    all: () => (sql.includes("FROM projects") ? TOPICS : []),
    get: () => null,
  }),
  run: () => { throw new Error("stub db.run"); },
};
export const todoRow = (id: number) => TODOS[id] ?? null;
export const isClaude = (id: string | null) => (id || "claude") === "claude";
export const DEFAULT_STARTED_PER_TURN = 10;
const no = (name: string) => () => { throw new Error(`stub ${name}`); };
export const addEvent = no("addEvent"), addMessage = no("addMessage"), attachmentsOf = no("attachmentsOf"), changed = no("changed"),
  commentsOf = no("commentsOf"), createTodo = no("createTodo"), filesOf = no("filesOf"), getMeta = no("getMeta"), liveSessionNo = no("liveSessionNo"),
  refreshPrs = no("refreshPrs"), removeTodo = no("removeTodo"), sessionsOf = no("sessionsOf"), setSchedule = no("setSchedule"), setTodo = no("setTodo"),
  summaries = no("summaries"), commitMessage = no("commitMessage"), isRunning = no("isRunning"), routeTo = no("routeTo"), setAutoEffort = no("setAutoEffort"),
  start = no("start");
