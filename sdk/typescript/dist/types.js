/**
 * Kleos SDK Types
 *
 * Type definitions matching the kleos-server API.
 */
/**
 * Custom error class for Kleos API errors.
 */
export class KleosError extends Error {
    statusCode;
    response;
    constructor(message, statusCode, response) {
        super(message);
        this.statusCode = statusCode;
        this.response = response;
        this.name = 'KleosError';
    }
}
//# sourceMappingURL=types.js.map