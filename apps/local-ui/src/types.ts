export type Card = {
  version: number;
  user_id: string;
  signing_key: string;
  curve_key: string;
  label: string;
  signature: string;
};
export type Topic = {
  id: string;
  peerId: string;
  title: string;
  createdAt: number;
  updatedAt: number;
  archived: boolean;
};
export type Message = {
  id: string;
  topicId: string;
  senderId: string;
  timestamp: number;
  body: string;
  format: 'markdown';
  replyTo?: string | null;
  delivery: 'queued' | 'sent' | 'delivered' | 'received';
};
export type Report = {
  sent: number;
  received: number;
  acknowledged: number;
  delivered: number;
  errors: string[];
};
export type Status = { protocol: number; lastSync: Report | null };
export type Outbox = {
  id: string;
  accepted: boolean;
  attempts: number;
  nextAttempt: number;
  lastError?: string | null;
};
