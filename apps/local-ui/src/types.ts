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
  delivery: 'queued' | 'sent' | 'delivered' | 'received' | 'failed';
};
export type Report = {
  sent: number;
  received: number;
  acknowledged: number;
  delivered: number;
  errors: string[];
};
export type DeviceCard = {
  version: number;
  id: string;
  label: string;
  signing_key: string;
  signature: string;
};
export type DeviceStatus = {
  card: DeviceCard;
  revoked: boolean;
  createdAt: number;
  lastSeen: number;
  acknowledgedCursor: number;
};
export type DeviceInfo = {
  card: DeviceCard;
  domainId: string;
  server: string;
  cursor: number;
  acknowledgedCursor: number;
};
export type Pending = {
  id: string;
  state: string;
  error: string | null;
  operation: { type: string; body?: string; title?: string; card?: Card };
};
export type Pairing = {
  version: number;
  device: DeviceCard;
  domain: Card;
  server: string;
  ca_pem: string;
  epoch: string;
  signature: string;
};
export type Status = {
  transport?: 'direct' | 'relay' | 'device';
  protocol: number;
  lastSync: Report | null;
  mode?: 'server' | 'client';
  device?: DeviceInfo | null;
  deviceServer?: string | null;
};
export type Outbox = {
  id: string;
  accepted: boolean;
  attempts: number;
  nextAttempt: number;
  lastError?: string | null;
};
