import { isValidPrinterId, parsePrinterLink } from './qr';

describe('isValidPrinterId', () => {
  it.each(['stand-001', 'A_b.c', '  x  '])('accepts %p', (id) => {
    expect(isValidPrinterId(id)).toBe(true);
  });
  it.each(['', '   ', 'a/b', 'a+b', 'a#b', 'a b', 'a?b'])(
    'rejects %p',
    (id) => {
      expect(isValidPrinterId(id)).toBe(false);
    },
  );
});

describe('parsePrinterLink', () => {
  it.each([
    ['https://fotoko.example/p/stand-001', 'stand-001'],
    ['https://fotoko.example/p/stand-001/', 'stand-001'],
    ['https://fotoko.example:8443/p/stand-001?x=1', 'stand-001'],
    ['https://fotoko.example/p/stand-001#frag', 'stand-001'],
    ['  https://FOTOKO.example/P/stand-001  ', 'stand-001'],
    ['fotoko://p/stand-001', 'stand-001'],
    ['fotoko://p/stand-001/', 'stand-001'],
    ['https://fotoko.example/p/stand%2D001', 'stand-001'],
  ])('parses %p', (link, id) => {
    expect(parsePrinterLink(link)).toBe(id);
  });

  it.each([
    '',
    'hello',
    'http://fotoko.example/p/stand-001',
    'https://fotoko.example/q/stand-001',
    'https://fotoko.example/p/',
    'https://fotoko.example/p',
    'https://fotoko.example/p/a/b',
    'https://fotoko.example/p/a%2Fb',
    'https://fotoko.example/p/a%23b',
    'https://fotoko.example/p/a%2Bb',
    'https://fotoko.example/p/%E0%A4%A',
    'https://fotoko.example/p/%20',
    'fotoko://q/stand-001',
    'fotoko://p/',
    'other://p/stand-001',
    'javascript:alert(1)',
  ])('rejects %p', (link) => {
    expect(parsePrinterLink(link)).toBeNull();
  });
});
