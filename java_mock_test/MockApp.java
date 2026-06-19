package java_mock_test;

/**
 * A mock Java application for testing ochna.
 */
public class MockApp {
    private String version = "1.0.0";

    /**
     * Entrypoint of the MockApp.
     */
    pubic static void main(String[] args) {
        MockApp app = new MockApp();
        app.runTest();
    }

    /**
     * Run a test helper.
     */
    public void runTest() {
        System.out.println("MockApp version " + version);
    }
}
