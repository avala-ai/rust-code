import 'package:agent_code_client/models/status_response.dart';
import 'package:test/test.dart';

void main() {
  group('StatusResponse.fromJson', () {
    test('parses complete response', () {
      final json = {
        'session_id': 'abc123',
        'model': 'test-model',
        'cwd': '/tmp/project',
        'turn_count': 5,
        'message_count': 10,
        'cost_usd': 0.0042,
        'plan_mode': false,
        'version': '0.13.1',
      };

      final status = StatusResponse.fromJson(json);
      expect(status.sessionId, 'abc123');
      expect(status.model, 'test-model');
      expect(status.cwd, '/tmp/project');
      expect(status.turnCount, 5);
      expect(status.messageCount, 10);
      expect(status.costUsd, closeTo(0.0042, 0.0001));
      expect(status.planMode, isFalse);
      expect(status.version, '0.13.1');
    });

    test('handles missing fields with defaults', () {
      final status = StatusResponse.fromJson({});
      expect(status.sessionId, '');
      expect(status.model, 'unknown');
      expect(status.turnCount, 0);
      expect(status.costUsd, 0.0);
      expect(status.planMode, isFalse);
    });
  });
}
