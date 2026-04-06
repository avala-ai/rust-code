import 'dart:convert';

import 'package:agent_code_client/models/json_rpc.dart';
import 'package:test/test.dart';

void main() {
  group('JsonRpcRequest', () {
    test('serializes to valid JSON-RPC 2.0', () {
      final req = JsonRpcRequest(id: 1, method: 'message', params: {'content': 'hello'});
      final json = jsonDecode(req.toJson()) as Map<String, dynamic>;

      expect(json['jsonrpc'], '2.0');
      expect(json['id'], 1);
      expect(json['method'], 'message');
      expect(json['params']['content'], 'hello');
    });

    test('omits params when empty', () {
      final req = JsonRpcRequest(id: 2, method: 'status');
      final json = jsonDecode(req.toJson()) as Map<String, dynamic>;

      expect(json.containsKey('params'), isFalse);
    });
  });

  group('JsonRpcResponse', () {
    test('serializes success response', () {
      final resp = JsonRpcResponse.success(1, {'data': 'ok'});
      final json = jsonDecode(resp.toJson()) as Map<String, dynamic>;

      expect(json['jsonrpc'], '2.0');
      expect(json['id'], 1);
      expect(json['result']['data'], 'ok');
      expect(json.containsKey('error'), isFalse);
    });

    test('serializes error response', () {
      final resp = JsonRpcResponse.error(1, -32600, 'Invalid');
      final json = jsonDecode(resp.toJson()) as Map<String, dynamic>;

      expect(json['error']['code'], -32600);
      expect(json['error']['message'], 'Invalid');
      expect(json.containsKey('result'), isFalse);
    });
  });

  group('JsonRpcNotification', () {
    test('serializes without id', () {
      final notif = JsonRpcNotification(method: 'events/text_delta', params: {'text': 'hi'});
      final json = jsonDecode(notif.toJson()) as Map<String, dynamic>;

      expect(json['jsonrpc'], '2.0');
      expect(json['method'], 'events/text_delta');
      expect(json.containsKey('id'), isFalse);
    });
  });

  group('parseJsonRpc', () {
    test('parses request (has id + method)', () {
      final raw = '{"jsonrpc":"2.0","id":1,"method":"message","params":{"content":"hi"}}';
      final result = parseJsonRpc(raw);

      expect(result, isA<JsonRpcRequest>());
      final req = result as JsonRpcRequest;
      expect(req.id, 1);
      expect(req.method, 'message');
      expect(req.params['content'], 'hi');
    });

    test('parses response (has id, no method)', () {
      final raw = '{"jsonrpc":"2.0","id":1,"result":{"ok":true}}';
      final result = parseJsonRpc(raw);

      expect(result, isA<JsonRpcResponse>());
      final resp = result as JsonRpcResponse;
      expect(resp.id, 1);
      expect(resp.result!['ok'], true);
      expect(resp.error, isNull);
    });

    test('parses error response', () {
      final raw = '{"jsonrpc":"2.0","id":1,"error":{"code":-32600,"message":"Bad"}}';
      final result = parseJsonRpc(raw);

      expect(result, isA<JsonRpcResponse>());
      final resp = result as JsonRpcResponse;
      expect(resp.error!.code, -32600);
    });

    test('parses notification (has method, no id)', () {
      final raw = '{"jsonrpc":"2.0","method":"events/text_delta","params":{"text":"x"}}';
      final result = parseJsonRpc(raw);

      expect(result, isA<JsonRpcNotification>());
      final notif = result as JsonRpcNotification;
      expect(notif.method, 'events/text_delta');
      expect(notif.params['text'], 'x');
    });

    test('throws on invalid message', () {
      expect(() => parseJsonRpc('{"jsonrpc":"2.0"}'), throwsFormatException);
    });

    test('handles null id as notification', () {
      final raw = '{"jsonrpc":"2.0","id":null,"method":"events/done","params":{}}';
      final result = parseJsonRpc(raw);
      expect(result, isA<JsonRpcNotification>());
    });
  });
}
