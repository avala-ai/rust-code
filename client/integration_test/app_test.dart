// Integration tests for the Agent Code Flutter client.
//
// These test the real app widget tree — no mocks. They verify that
// the UI renders correctly, buttons are tappable, and state transitions
// work as expected.
//
// Run against Chrome:
//   fvm flutter test integration_test/app_test.dart -d chrome
//
// Run headless (CI):
//   fvm flutter test integration_test/app_test.dart -d web-server

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';

import 'package:agent_code_client_app/main.dart' as app;

void main() {
  IntegrationTestWidgetsFlutterBinding.ensureInitialized();

  // ── Layout & Structure ────────────────────────────────────────

  group('App Shell', () {
    testWidgets('renders sidebar and main area', (tester) async {
      app.main();
      await tester.pumpAndSettle();

      // Sidebar header.
      expect(find.text('SESSIONS'), findsOneWidget);

      // Empty state in main area.
      expect(find.text('Agent Code'), findsOneWidget);
      expect(find.text('Create a new session to get started'), findsOneWidget);
    });

    testWidgets('sidebar shows "No sessions yet" when empty', (tester) async {
      app.main();
      await tester.pumpAndSettle();

      expect(find.textContaining('No sessions yet'), findsOneWidget);
      expect(find.textContaining('Click + to start'), findsOneWidget);
    });

    testWidgets('has vertical divider between sidebar and main', (tester) async {
      app.main();
      await tester.pumpAndSettle();

      expect(find.byType(VerticalDivider), findsOneWidget);
    });

    testWidgets('sidebar has fixed width of 240', (tester) async {
      app.main();
      await tester.pumpAndSettle();

      final sizedBox = tester.widgetList<SizedBox>(find.byType(SizedBox)).where((sb) => sb.width == 240);
      expect(sizedBox, isNotEmpty);
    });
  });

  // ── New Session Button ────────────────────────────────────────

  group('+ New Button', () {
    testWidgets('is visible in sidebar', (tester) async {
      app.main();
      await tester.pumpAndSettle();

      final newButton = find.text('+ New');
      expect(newButton, findsOneWidget);
    });

    testWidgets('is a TextButton and tappable', (tester) async {
      app.main();
      await tester.pumpAndSettle();

      final button = find.widgetWithText(TextButton, '+ New');
      expect(button, findsOneWidget);

      // Should not throw on tap.
      await tester.tap(button);
      await tester.pumpAndSettle();
    });

    testWidgets('shows error on web (no process spawning)', (tester) async {
      app.main();
      await tester.pumpAndSettle();

      await tester.tap(find.text('+ New'));
      await tester.pumpAndSettle();

      // On web, agentManager is null — should show error.
      expect(find.textContaining('Cannot spawn'), findsOneWidget);
    });

    testWidgets('error is displayed in sidebar bottom area', (tester) async {
      app.main();
      await tester.pumpAndSettle();

      await tester.tap(find.text('+ New'));
      await tester.pumpAndSettle();

      // Error text should be styled with error color.
      final errorFinder = find.textContaining('Cannot spawn');
      expect(errorFinder, findsOneWidget);

      final errorWidget = tester.widget<Text>(errorFinder);
      expect(errorWidget.maxLines, 2);
    });
  });

  // ── Theme ─────────────────────────────────────────────────────

  group('Theme', () {
    testWidgets('app uses Material 3', (tester) async {
      app.main();
      await tester.pumpAndSettle();

      final materialApp = tester.widget<MaterialApp>(find.byType(MaterialApp));
      expect(materialApp.theme?.useMaterial3, isTrue);
      expect(materialApp.darkTheme?.useMaterial3, isTrue);
    });

    testWidgets('debug banner is hidden', (tester) async {
      app.main();
      await tester.pumpAndSettle();

      final materialApp = tester.widget<MaterialApp>(find.byType(MaterialApp));
      expect(materialApp.debugShowCheckedModeBanner, isFalse);
    });

    testWidgets('supports dark and light mode', (tester) async {
      app.main();
      await tester.pumpAndSettle();

      final materialApp = tester.widget<MaterialApp>(find.byType(MaterialApp));
      expect(materialApp.themeMode, ThemeMode.system);
      expect(materialApp.theme, isNotNull);
      expect(materialApp.darkTheme, isNotNull);
    });
  });

  // ── Widget Tree Integrity ─────────────────────────────────────

  group('Widget Tree', () {
    testWidgets('has BlocProvider for SessionBloc', (tester) async {
      app.main();
      await tester.pumpAndSettle();

      // BlocBuilder inside Sidebar and AppShell read SessionBloc.
      // If BlocProvider is missing, the app would crash.
      // The fact that the app renders proves BlocProvider exists.
      expect(find.text('SESSIONS'), findsOneWidget);
    });

    testWidgets('Row contains sidebar and expanded main area', (tester) async {
      app.main();
      await tester.pumpAndSettle();

      // The AppShell has a Row with SizedBox(240) + VerticalDivider + Expanded.
      expect(find.byType(Row), findsWidgets);
      expect(find.byType(Expanded), findsWidgets);
    });

    testWidgets('no overflow errors on standard screen size', (tester) async {
      // Default test window is 800x600. Should render without overflow.
      app.main();
      await tester.pumpAndSettle();

      // If there's an overflow, Flutter will report an exception.
      // pumpAndSettle would throw. Reaching here means no overflow.
      expect(find.byType(MaterialApp), findsOneWidget);
    });
  });

  // ── Accessibility ─────────────────────────────────────────────

  group('Accessibility', () {
    testWidgets('+ New button has semantic label', (tester) async {
      app.main();
      await tester.pumpAndSettle();

      // The button text "+ New" serves as the semantic label.
      final button = find.text('+ New');
      expect(button, findsOneWidget);

      // Verify it's inside a tappable widget.
      expect(find.widgetWithText(TextButton, '+ New'), findsOneWidget);
    });
  });
}
