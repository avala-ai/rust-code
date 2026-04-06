import 'package:agent_code_client/models/chat_message.dart';
import 'package:test/test.dart';

void main() {
  group('ChatMessage', () {
    test('user message has correct role', () {
      final msg = ChatMessage.user('hello');
      expect(msg.role, 'user');
      expect(msg.content, 'hello');
      expect(msg.id, isNotEmpty);
    });

    test('assistant message starts empty', () {
      final msg = ChatMessage.assistant();
      expect(msg.role, 'assistant');
      expect(msg.content, '');
      expect(msg.toolCalls, isEmpty);
      expect(msg.thinking, isNull);
    });

    test('generates unique IDs', () {
      final a = ChatMessage.user('a');
      final b = ChatMessage.user('b');
      expect(a.id, isNot(equals(b.id)));
    });

    test('content is mutable for streaming appends', () {
      final msg = ChatMessage.assistant();
      msg.content += 'hello';
      msg.content += ' world';
      expect(msg.content, 'hello world');
    });
  });

  group('ToolCall', () {
    test('defaults to running status', () {
      final tool = ToolCall(name: 'bash');
      expect(tool.status, ToolCallStatus.running);
      expect(tool.id, isNotEmpty);
    });

    test('status is mutable', () {
      final tool = ToolCall(name: 'bash');
      tool.status = ToolCallStatus.done;
      expect(tool.status, ToolCallStatus.done);
    });

    test('generates unique IDs', () {
      final a = ToolCall(name: 'bash');
      final b = ToolCall(name: 'bash');
      expect(a.id, isNot(equals(b.id)));
    });
  });
}
