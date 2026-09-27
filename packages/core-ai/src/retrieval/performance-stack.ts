import type { FileChunk } from '@agentcowork/core-domain';

export type TwoTierSearchFn = (query: string, fileId: string) => Promise<FileChunk[]>;

export type ReaderPerformanceStackOptions = {
  /** Injected search (e.g. core-files twoTierFileSearch bound to a db handle). */
  searchFn?: TwoTierSearchFn;
};

export type ChaptersReadyState = {
  chaptersReady: number;
  chaptersTotal: number;
};

export type LazyChapterIngestOptions = {
  fileId: string;
  chapters: string[];
  readingPosition?: number;
  onChapterEmbedded?: (chapterIndex: number) => void;
};

const QUESTION_CACHE_TTL_MS = 24 * 60 * 60 * 1000;

export class QuestionCache {
  private readonly entries = new Map<string, { answer: string; expiresAt: number }>();

  private key(question: string, bookId: string): string {
    let hash = 0;
    const input = `${bookId}::${question.trim().toLowerCase()}`;
    for (let i = 0; i < input.length; i += 1) {
      hash = (hash * 31 + input.charCodeAt(i)) >>> 0;
    }
    return hash.toString(16);
  }

  get(question: string, bookId: string): string | undefined {
    const entry = this.entries.get(this.key(question, bookId));
    if (!entry || entry.expiresAt <= Date.now()) {
      return undefined;
    }
    return entry.answer;
  }

  set(question: string, bookId: string, answer: string): void {
    this.entries.set(this.key(question, bookId), {
      answer,
      expiresAt: Date.now() + QUESTION_CACHE_TTL_MS,
    });
  }

  clear(): void {
    this.entries.clear();
  }
}

export class ReaderPerformanceStack {
  private readonly searchFn: TwoTierSearchFn | undefined;
  readonly questionCache = new QuestionCache();
  private readonly chaptersReadyByFile = new Map<string, ChaptersReadyState>();
  /** Precomputed chapter summaries: fileId → chapterId → summary text. */
  private readonly chapterSummaries = new Map<string, Map<string, string>>();

  constructor(options: ReaderPerformanceStackOptions = {}) {
    this.searchFn = options.searchFn;
  }

  /**
   * Store a precomputed chapter summary (from index pipeline or offline job).
   */
  setChapterSummary(fileId: string, chapterId: string, summary: string): void {
    let byChapter = this.chapterSummaries.get(fileId);
    if (!byChapter) {
      byChapter = new Map();
      this.chapterSummaries.set(fileId, byChapter);
    }
    byChapter.set(chapterId, summary.trim());
  }

  getChapterSummary(fileId: string, chapterId: string): string | undefined {
    return this.chapterSummaries.get(fileId)?.get(chapterId);
  }

  /**
   * Two-tier retrieval as per spec §7.6.
   * FTS5 BM25 top-50 (~4ms) → sqlite-vec rerank top-5 (~8ms).
   */
  async twoTierRetrieve(query: string, fileId: string): Promise<FileChunk[]> {
    if (!this.searchFn) {
      return [];
    }
    return this.searchFn(query, fileId);
  }

  /**
   * Lazy chapter ingest — prioritize chapters near reading position.
   * Updates chaptersReady/total UI state for the reader banner.
   */
  async lazyChapterIngest(options: LazyChapterIngestOptions): Promise<ChaptersReadyState> {
    const { fileId, chapters, readingPosition = 0, onChapterEmbedded } = options;
    const total = chapters.length;
    const prioritized = chapters
      .map((chapter, index) => ({ chapter, index }))
      .sort((a, b) => Math.abs(a.index - readingPosition) - Math.abs(b.index - readingPosition));

    let ready = 0;
    for (const item of prioritized) {
      onChapterEmbedded?.(item.index);
      ready += 1;
      this.chaptersReadyByFile.set(fileId, { chaptersReady: ready, chaptersTotal: total });
    }

    const state = { chaptersReady: ready, chaptersTotal: total };
    this.chaptersReadyByFile.set(fileId, state);
    return state;
  }

  getChaptersReady(fileId: string): ChaptersReadyState | undefined {
    return this.chaptersReadyByFile.get(fileId);
  }

  /**
   * Precomputed summary layer routing.
   * Returns cached summary when present; otherwise a clear miss message
   * so callers can escalate to an LLM summarize step.
   */
  async routeSummaryQuery(
    _query: string,
    chapterId: string,
    fileId?: string,
  ): Promise<{ summary: string; hit: boolean }> {
    if (fileId) {
      const cached = this.getChapterSummary(fileId, chapterId);
      if (cached) {
        return { summary: cached, hit: true };
      }
    }
    // Scan all files if fileId not provided (small maps; reader usually has one book).
    for (const byChapter of this.chapterSummaries.values()) {
      const cached = byChapter.get(chapterId);
      if (cached) return { summary: cached, hit: true };
    }
    return {
      summary: `No precomputed summary for chapter ${chapterId}.`,
      hit: false,
    };
  }

  /**
   * 20-line model router (spec §7.6).
   * Classifies question complexity to route to the right model tier.
   */
  routeQuery(query: string): 'define' | 'summarize' | 'qa' | 'synthesis' {
    const q = query.toLowerCase();
    if (q.startsWith('who is') || q.startsWith('what is') || q.startsWith('define')) {
      return 'define';
    }
    if (q.includes('summarize') || q.includes('summary')) {
      return 'summarize';
    }
    if (q.includes('compare') || q.includes('across chapters')) {
      return 'synthesis';
    }
    return 'qa';
  }

  /**
   * Select first-token latency target by question type (spec §7.6).
   * Used by the reader to show progress indicators.
   */
  getExpectedLatency(type: ReturnType<typeof this.routeQuery>): 'fast' | 'medium' | 'slow' {
    switch (type) {
      case 'define':
        return 'fast'; // On-device SLM ~250ms
      case 'summarize':
        return 'fast'; // Summary layer ~600ms
      case 'qa':
        return 'medium'; // Cloud BYOK ~1.2s
      case 'synthesis':
        return 'slow'; // Multi-chapter ~3s
    }
  }
}