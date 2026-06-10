/**
 * 编译效率三件套:debounce + cancel(AbortController)+ hash-cache。
 * 泛型设计,compile-engine 与 preview-engine L2 共用。
 */

export interface QueueResult<T> {
  result: T;
  /** 命中缓存(未实际执行 run)。 */
  cached: boolean;
  key: string;
}

export interface DebounceQueueOptions<J, T> {
  /** 防抖窗口(ms)。规格:300–500。 */
  debounceMs?: number;
  /** 实际执行体;收到 signal,被取消时应尽快中止(如 kill soffice)。 */
  run: (job: J, signal: AbortSignal) => Promise<T>;
  /** 由 job 计算缓存键(通常含 content-hash + format)。 */
  keyOf: (job: J) => string;
  /** 缓存最大条目数(FIFO 淘汰)。 */
  maxCache?: number;
}

interface Waiter<T> {
  resolve: (r: QueueResult<T>) => void;
  reject: (e: unknown) => void;
}

export class DebounceQueue<J, T> {
  private readonly debounceMs: number;
  private readonly run: (job: J, signal: AbortSignal) => Promise<T>;
  private readonly keyOf: (job: J) => string;
  private readonly maxCache: number;

  private readonly cache = new Map<string, T>();
  private waiters: Waiter<T>[] = [];
  private latestJob?: J;
  private timer?: ReturnType<typeof setTimeout>;
  private runningAbort?: AbortController;
  private runningWaiters?: Waiter<T>[];

  /** 统计:实际编译次数 / 缓存命中 / 取消次数。 */
  readonly stats = { compiles: 0, cacheHits: 0, cancels: 0, debounced: 0 };

  constructor(options: DebounceQueueOptions<J, T>) {
    this.debounceMs = options.debounceMs ?? 400;
    this.run = options.run;
    this.keyOf = options.keyOf;
    this.maxCache = options.maxCache ?? 64;
  }

  /** 提交一个 job。返回最终(去抖后最新)结果。 */
  schedule(job: J): Promise<QueueResult<T>> {
    return new Promise<QueueResult<T>>((resolve, reject) => {
      this.latestJob = job;
      this.waiters.push({ resolve, reject });

      // 取消在飞任务:其等待者并入新批,稍后用最新结果兑现
      if (this.runningAbort) {
        this.runningAbort.abort();
        this.stats.cancels++;
        if (this.runningWaiters) {
          this.waiters.push(...this.runningWaiters);
          this.runningWaiters = undefined;
        }
        this.runningAbort = undefined;
      }

      if (this.timer) {
        clearTimeout(this.timer);
        this.stats.debounced++;
      }
      this.timer = setTimeout(() => this.fire(), this.debounceMs);
    });
  }

  /** 立即执行(跳过防抖),用于一次性导出。 */
  async runNow(job: J): Promise<QueueResult<T>> {
    const key = this.keyOf(job);
    if (this.cache.has(key)) {
      this.stats.cacheHits++;
      return { result: this.cache.get(key)!, cached: true, key };
    }
    const ac = new AbortController();
    this.stats.compiles++;
    const result = await this.run(job, ac.signal);
    this.store(key, result);
    return { result, cached: false, key };
  }

  has(key: string): boolean {
    return this.cache.has(key);
  }

  clearCache(): void {
    this.cache.clear();
  }

  /** 取消挂起/在飞任务(等待者将被 reject)。 */
  dispose(): void {
    if (this.timer) clearTimeout(this.timer);
    if (this.runningAbort) this.runningAbort.abort();
    const all = [...this.waiters, ...(this.runningWaiters ?? [])];
    this.waiters = [];
    this.runningWaiters = undefined;
    for (const w of all) w.reject(new Error("queue disposed"));
  }

  private fire(): void {
    this.timer = undefined;
    if (this.waiters.length === 0 || this.latestJob === undefined) return;
    const batch = this.waiters;
    this.waiters = [];
    const job = this.latestJob;
    const key = this.keyOf(job);

    // hash-cache 命中:直接返回,跳过编译
    if (this.cache.has(key)) {
      this.stats.cacheHits++;
      const result = this.cache.get(key)!;
      for (const w of batch) w.resolve({ result, cached: true, key });
      return;
    }

    const ac = new AbortController();
    this.runningAbort = ac;
    this.runningWaiters = batch;
    this.stats.compiles++;

    this.run(job, ac.signal).then(
      (result) => {
        if (ac.signal.aborted) return; // 被新任务取代,等待者已并入新批
        this.runningAbort = undefined;
        this.runningWaiters = undefined;
        this.store(key, result);
        for (const w of batch) w.resolve({ result, cached: false, key });
      },
      (err) => {
        if (ac.signal.aborted) return; // 取消导致的报错忽略
        this.runningAbort = undefined;
        this.runningWaiters = undefined;
        for (const w of batch) w.reject(err);
      },
    );
  }

  private store(key: string, value: T): void {
    this.cache.set(key, value);
    while (this.cache.size > this.maxCache) {
      const first = this.cache.keys().next().value;
      if (first === undefined) break;
      this.cache.delete(first);
    }
  }
}
