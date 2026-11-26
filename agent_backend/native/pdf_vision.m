#import <CoreGraphics/CoreGraphics.h>
#import <Foundation/Foundation.h>
#import <ImageIO/ImageIO.h>
#import <PDFKit/PDFKit.h>
#import <Vision/Vision.h>
#include <math.h>
#include <string.h>

static CGContextRef pageBitmap(CGPDFPageRef page, size_t longEdge, BOOL color,
                               NSArray<NSValue *> *covered, CGAffineTransform *transform) {
    if (!page) return NULL;
    CGRect crop = CGPDFPageGetBoxRect(page, kCGPDFCropBox);
    CGFloat width = crop.size.width, height = crop.size.height;
    if (CGPDFPageGetRotationAngle(page) % 180 != 0) {
        width = crop.size.height;
        height = crop.size.width;
    }
    if (!(width > 0 && height > 0 && isfinite(width) && isfinite(height))) return NULL;
    CGFloat scale = longEdge / MAX(width, height);
    size_t w = MAX(1, round(width * scale)), h = MAX(1, round(height * scale));
    CGRect bounds = CGRectMake(0, 0, w, h);
    CGColorSpaceRef space = color ? CGColorSpaceCreateDeviceRGB() : CGColorSpaceCreateDeviceGray();
    CGBitmapInfo format = (CGBitmapInfo)(color ? kCGImageAlphaNoneSkipLast : kCGImageAlphaNone);
    CGContextRef context = CGBitmapContextCreate(NULL, w, h, 8, 0, space, format);
    CGColorSpaceRelease(space);
    if (!context) return NULL;
    CGContextSetGrayFillColor(context, 1, 1);
    CGContextFillRect(context, bounds);
    CGAffineTransform drawing = CGPDFPageGetDrawingTransform(page, kCGPDFCropBox, bounds, 0, true);
    if (transform) *transform = drawing;
    CGContextConcatCTM(context, drawing);
    CGContextDrawPDFPage(context, page);
    CGContextSetGrayFillColor(context, 1, 1);
    for (NSValue *value in covered) {
        CGContextFillRect(context, CGRectInset(value.rectValue, -0.75, -0.75));
    }
    return context;
}

// Mask individual words so that an equation image between two words on a line
// remains visible to OCR. PDFKit and the renderer use the same page coordinates.
static NSArray<NSValue *> *wordBounds(PDFPage *page, NSString *text) {
    NSUInteger length = MIN(text.length, page.numberOfCharacters);
    NSMutableArray<NSValue *> *bounds = [NSMutableArray array];
    NSRegularExpression *words = [NSRegularExpression regularExpressionWithPattern:@"\\S+" options:0 error:NULL];
    [words enumerateMatchesInString:text options:0 range:NSMakeRange(0, length)
                         usingBlock:^(NSTextCheckingResult *word, __unused NSMatchingFlags flags, __unused BOOL *stop) {
        PDFSelection *selection = [page selectionForRange:word.range];
        if (!selection) return;
        CGRect rect = [selection boundsForPage:page];
        if (!CGRectIsEmpty(rect) && !CGRectIsInfinite(rect)) [bounds addObject:[NSValue valueWithRect:rect]];
    }];
    return bounds;
}

// A small masked render also finds vector diagrams that have no image resource.
static BOOL hasUncoveredInk(PDFPage *page, NSArray<NSValue *> *covered) {
    CGContextRef bitmap = pageBitmap(page.pageRef, 520, NO, covered, NULL);
    if (!bitmap) return YES;
    const uint8_t *data = CGBitmapContextGetData(bitmap);
    size_t w = CGBitmapContextGetWidth(bitmap), h = CGBitmapContextGetHeight(bitmap);
    size_t stride = CGBitmapContextGetBytesPerRow(bitmap);
    size_t ink = 0;
    for (size_t y = 0; y < h && ink < 16; y++) {
        const uint8_t *row = data + y * stride;
        for (size_t x = 0; x < w && ink < 16; x++) ink += row[x] < 205;
    }
    CGContextRelease(bitmap);
    return ink >= 16;
}

static NSString *recognizePage(PDFPage *page, NSString *native, NSArray<NSValue *> *covered,
                               NSString **problem) API_AVAILABLE(macos(10.15));
static NSString *recognizePage(PDFPage *page, NSString *native, NSArray<NSValue *> *covered,
                               NSString **problem) {
    CGAffineTransform drawing;
    CGContextRef bitmap = pageBitmap(page.pageRef, 2400, NO, covered, &drawing);
    if (!bitmap) {
        *problem = @"页面无法渲染";
        return native;
    }
    CGFloat width = CGBitmapContextGetWidth(bitmap), height = CGBitmapContextGetHeight(bitmap);
    CGImageRef image = CGBitmapContextCreateImage(bitmap);
    CGContextRelease(bitmap);
    if (!image) {
        *problem = @"页面无法渲染";
        return native;
    }
    VNImageRequestHandler *handler = [[VNImageRequestHandler alloc] initWithCGImage:image options:@{}];
    CGImageRelease(image);
    VNRecognizeTextRequest *request = [VNRecognizeTextRequest new];
    request.recognitionLevel = VNRequestTextRecognitionLevelAccurate;
    if (@available(macOS 11.0, *)) request.recognitionLanguages = @[@"zh-Hans", @"en-US"];
    NSError *error = nil;
    if (![handler performRequests:@[request] error:&error]) {
        *problem = error.localizedDescription ?: @"Vision 识别失败";
        return native;
    }
    // Insert new lines where they occur on the page. Keep the original PDF
    // string byte-for-byte between insertions, including its math and columns.
    NSArray<PDFSelection *> *lines = native.length
        ? [page selectionForRange:NSMakeRange(0, page.numberOfCharacters)].selectionsByLine : @[];
    NSMutableDictionary<NSNumber *, NSMutableArray<NSString *> *> *insertions = [NSMutableDictionary dictionary];
    NSMutableIndexSet *inlineOffsets = [NSMutableIndexSet indexSet];
    for (VNRecognizedTextObservation *observation in request.results) {
        NSString *text = [observation topCandidates:1].firstObject.string;
        if (!text.length) continue;
        CGRect box = CGRectApplyAffineTransform(observation.boundingBox, CGAffineTransformMakeScale(width, height));
        NSUInteger offset = native.length;
        for (PDFSelection *line in lines) {
            if (![line numberOfTextRangesOnPage:page]) continue;
            NSRange range = NSIntersectionRange([line rangeAtIndex:0 onPage:page], NSMakeRange(0, native.length));
            if (!range.length) continue;
            CGRect rect = CGRectApplyAffineTransform([line boundsForPage:page], drawing);
            CGFloat delta = CGRectGetMidY(box) - CGRectGetMidY(rect);
            CGFloat tolerance = MIN(box.size.height, rect.size.height) / 2;
            if (fabs(delta) <= tolerance && CGRectIntersectsRect(CGRectInset(rect, -rect.size.height, 0), box)) {
                // An image can occupy a gap inside a sentence. Insert before
                // the following glyph instead of moving it to the end of the page.
                offset = NSMaxRange(range);
                for (NSUInteger j = range.location; j < NSMaxRange(range); j++) {
                    CGRect glyph = CGRectApplyAffineTransform([page characterBoundsAtIndex:j], drawing);
                    if (!CGRectIsEmpty(glyph) && CGRectGetMidX(glyph) > CGRectGetMidX(box)) {
                        offset = j;
                        break;
                    }
                }
                [inlineOffsets addIndex:offset];
                break;
            }
            if (delta > tolerance || (fabs(delta) <= tolerance && CGRectGetMaxX(box) <= CGRectGetMinX(rect))) {
                offset = range.location;
                break;
            }
        }
        if (!insertions[@(offset)]) insertions[@(offset)] = [NSMutableArray array];
        [insertions[@(offset)] addObject:text];
    }
    NSMutableString *merged = [native mutableCopy];
    NSArray<NSNumber *> *offsets = [insertions.allKeys sortedArrayUsingSelector:@selector(compare:)];
    for (NSNumber *offset in offsets.reverseObjectEnumerator) {
        NSUInteger index = offset.unsignedIntegerValue;
        BOOL inLine = [inlineOffsets containsIndex:index];
        NSString *separator = inLine ? @" " : @"\n";
        NSString *addition = [insertions[offset] componentsJoinedByString:separator];
        NSCharacterSet *spacing = inLine ? NSCharacterSet.whitespaceAndNewlineCharacterSet : NSCharacterSet.newlineCharacterSet;
        BOOL prefix = index && ![spacing characterIsMember:[native characterAtIndex:index - 1]];
        BOOL suffix = index < native.length && ![spacing characterIsMember:[native characterAtIndex:index]];
        [merged insertString:[NSString stringWithFormat:@"%@%@%@", prefix ? separator : @"", addition,
                              suffix ? separator : @""] atIndex:index];
    }
    return merged;
}

static PDFDocument *openPDF(const char *path) {
    if (!path) return nil;
    PDFDocument *pdf = [[PDFDocument alloc] initWithURL:[NSURL fileURLWithPath:@(path)]];
    return pdf.isLocked ? nil : pdf;
}

size_t jisjtu_pdf_page_count(const char *path) {
    @autoreleasepool { return openPDF(path).pageCount; }
}

char *jisjtu_pdf_page_jpeg(const char *path, size_t pageNumber) {
    @autoreleasepool {
        PDFDocument *pdf = openPDF(path);
        if (pageNumber == 0 || pageNumber > pdf.pageCount) return NULL;
        CGContextRef bitmap = pageBitmap([pdf pageAtIndex:pageNumber - 1].pageRef, 1700, YES, nil, NULL);
        if (!bitmap) return NULL;
        CGImageRef image = CGBitmapContextCreateImage(bitmap);
        CGContextRelease(bitmap);
        if (!image) return NULL;
        NSMutableData *data = [NSMutableData data];
        CGImageDestinationRef jpeg = CGImageDestinationCreateWithData((__bridge CFMutableDataRef)data,
                                                                      CFSTR("public.jpeg"), 1, NULL);
        BOOL ok = NO;
        if (jpeg) {
            NSDictionary *options = @{(__bridge NSString *)kCGImageDestinationLossyCompressionQuality: @0.85};
            CGImageDestinationAddImage(jpeg, image, (__bridge CFDictionaryRef)options);
            ok = CGImageDestinationFinalize(jpeg);
            CFRelease(jpeg);
        }
        CGImageRelease(image);
        return ok ? strdup([data base64EncodedStringWithOptions:0].UTF8String) : NULL;
    }
}

// A successful call returns one string per page. The Rust caller owns the output
// or error buffer. PDFKit can retain its copy of the snapshot independently.
char *jisjtu_pdf_pages(const uint8_t *bytes, size_t length, char **error) {
    @autoreleasepool {
        NSData *data = [NSData dataWithBytes:bytes length:length];
        PDFDocument *pdf = [[PDFDocument alloc] initWithData:data];
        if (!pdf || pdf.isLocked) {
            *error = strdup(pdf ? "PDF 已加密，请先解锁" : "无法打开 PDF 文件");
            return NULL;
        }
        NSMutableArray<NSString *> *pages = [NSMutableArray arrayWithCapacity:pdf.pageCount];
        NSString *firstNotice = nil;
        BOOL anyText = NO;
        for (NSUInteger i = 0; i < pdf.pageCount; i++) {
            @autoreleasepool {
                PDFPage *page = [pdf pageAtIndex:i];
                NSString *text = page.string ?: @"";
                NSArray<NSValue *> *covered = wordBounds(page, text);
                NSString *notice = nil;
                if (hasUncoveredInk(page, covered)) {
                    NSString *problem = nil;
                    if (@available(macOS 10.15, *)) {
                        text = recognizePage(page, text, covered, &problem);
                    } else {
                        problem = @"离线 OCR 需要 macOS 10.15 或更高版本";
                    }
                    NSString *reason = problem ? [NSString stringWithFormat:@"图像文字未成功识别（%@）", problem]
                                               : @"含图像或图形，公式、图表布局需核对原页";
                    notice = [NSString stringWithFormat:@"[第 %lu 页%@%@。查看原页可用 pdf_analysis 的 image_page=%lu]",
                              (unsigned long)i + 1, reason, problem ? @"，请核对原页" : @"", (unsigned long)i + 1];
                    if (!firstNotice) firstNotice = notice;
                }
                if ([text stringByTrimmingCharactersInSet:NSCharacterSet.whitespaceAndNewlineCharacterSet].length) anyText = YES;
                [pages addObject:notice ? [text stringByAppendingFormat:@"\n%@", notice] : text];
            }
        }
        if (!anyText && firstNotice) {
            *error = strdup(firstNotice.UTF8String);
            return NULL;
        }
        NSData *result = [NSJSONSerialization dataWithJSONObject:pages options:0 error:NULL];
        if (!result) {
            *error = strdup("无法编码页面文字");
            return NULL;
        }
        return strndup(result.bytes, result.length);
    }
}
