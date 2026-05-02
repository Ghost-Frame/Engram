/**
 * Engram SDK Types
 *
 * Type definitions matching the engram-server API.
 */
/**
 * Custom error class for Engram API errors.
 */
export class EngramError extends Error {
    statusCode;
    response;
    constructor(message, statusCode, response) {
        super(message);
        this.statusCode = statusCode;
        this.response = response;
        this.name = 'EngramError';
    }
}
//# sourceMappingURL=types.js.map