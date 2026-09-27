import type { SearchContext, SearchProvider } from '@agentcowork/core-domain';

export class WebFetchCascade {
  private providers: SearchProvider[] = [];

  constructor(providers: SearchProvider[]) {
    this.providers = providers.filter(p => p.kind === 'fetch');
  }

  async fetch(url: string, ctx: SearchContext): Promise<string> {
    for (const provider of this.providers) {
      if (!provider.fetch) continue;
      
      try {
        const isAvailable = await provider.isAvailable(ctx);
        if (!isAvailable) continue;

        const content = await provider.fetch(url);
        if (content && content.trim().length > 0) {
          return content;
        }
      } catch (error) {
        console.warn(`[WebFetchCascade] Provider ${provider.name} failed:`, error);
        // Continue to the next provider
      }
    }

    throw new Error('Failed to fetch URL content via all available providers in the cascade.');
  }
}
