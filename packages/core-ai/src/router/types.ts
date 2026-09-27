import type {
  IntentCategory,
  IntentClassification,
  RouteContext,
  RouteDecision,
  UserQuery,
} from '@agentcowork/core-domain';

export type { IntentCategory, IntentClassification, RouteContext, RouteDecision, UserQuery };

export interface IntentClassifier {
  classify(query: UserQuery): Promise<IntentClassification>;
}

export interface SmartRouterOptions {
  classifier: IntentClassifier;
}