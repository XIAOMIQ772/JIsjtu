#import <CoreGraphics/CoreGraphics.h>
#import <Foundation/Foundation.h>
#import <PDFKit/PDFKit.h>
#import <Vision/Vision.h>
#include <math.h>
#include <string.h>

static CGImageRef renderPage(CGPDFPageRef page) {
    if (!page) return NULL;
    CGRect crop = CGPDFPageGetBoxRect(page, kCGPDFCropBox);
    CGFloat width = crop.size.width, height = crop.size.height;
    if (CGPDFPageGetRotationAngle(page) % 180 != 0) {
        width = crop.size.height;
        height = crop.size.width;
    }
    if (!(width > 0 && height > 0 && isfinite(width) && isfinite(height))) return NULL;
    CGFloat longest = MAX(width, height);
    size_t pixelsWide = MAX(1, round(width / longest * 2400));
    size_t pixelsHigh = MAX(1, round(height / longest * 2400));
    CGRect bounds = CGRectMake(0, 0, pixelsWide, pixelsHigh);
    CGColorSpaceRef gray = CGColorSpaceCreateDeviceGray();
    CGContextRef context = CGBitmapContextCreate(NULL, pixelsWide, pixelsHigh, 8, 0, gray, (CGBitmapInfo)kCGImageAlphaNone);
    CGColorSpaceRelease(gray);
    if (!context) return NULL;
    CGContextSetGrayFillColor(context, 1, 1);
    CGContextFillRect(context, bounds);
    CGContextConcatCTM(context, CGPDFPageGetDrawingTransform(page, kCGPDFCropBox, bounds, 0, true));
    CGContextDrawPDFPage(context, page);
    CGImageRef image = CGBitmapContextCreateImage(context);
    CGContextRelease(context);
    return image;
}

static NSString *recognizePage(PDFPage *page, NSString **problem) API_AVAILABLE(macos(10.15));
static NSString *recognizePage(PDFPage *page, NSString **problem) {
    CGImageRef image = renderPage(page.pageRef);
    if (!image) {
        *problem = @"页面无法渲染";
        return nil;
    }
    VNImageRequestHandler *handler = [[VNImageRequestHandler alloc] initWithCGImage:image options:@{}];
    CGImageRelease(image);
    VNRecognizeTextRequest *request = [VNRecognizeTextRequest new];
    request.recognitionLevel = VNRequestTextRecognitionLevelAccurate;
    if (@available(macOS 11.0, *)) {
        request.recognitionLanguages = @[@"zh-Hans", @"en-US"];
    }
    NSError *error = nil;
    if (![handler performRequests:@[request] error:&error]) {
        *problem = error.localizedDescription ?: @"Vision 识别失败";
        return nil;
    }
    NSMutableArray<NSString *> *lines = [NSMutableArray array];
    for (VNRecognizedTextObservation *observation in request.results) {
        NSString *text = [observation topCandidates:1].firstObject.string;
        if (text.length) [lines addObject:text];
    }
    return [lines componentsJoinedByString:@"\n"];
}

// A successful call returns a JSON array with one string per page. The caller
// owns the returned buffer, or the error buffer on failure.
char *jisjtu_pdf_pages(const char *path, char **error) {
    @autoreleasepool {
        PDFDocument *pdf = [[PDFDocument alloc] initWithURL:[NSURL fileURLWithPath:@(path)]];
        if (!pdf || pdf.isLocked) {
            *error = strdup(pdf ? "PDF 已加密，请先解锁" : "无法打开 PDF 文件");
            return NULL;
        }
        NSMutableArray<NSString *> *pages = [NSMutableArray arrayWithCapacity:pdf.pageCount];
        for (NSUInteger i = 0; i < pdf.pageCount; i++) {
            @autoreleasepool {
                PDFPage *page = [pdf pageAtIndex:i];
                NSString *text = page.string;
                if (![text stringByTrimmingCharactersInSet:NSCharacterSet.whitespaceAndNewlineCharacterSet].length) {
                    NSString *problem = nil;
                    if (@available(macOS 10.15, *)) {
                        text = recognizePage(page, &problem);
                    } else {
                        problem = @"扫描页 OCR 需要 macOS 10.15 或更高版本";
                    }
                    if (problem) {
                        NSString *message = [NSString stringWithFormat:@"第 %lu 页：%@", (unsigned long)i + 1, problem];
                        *error = strdup(message.UTF8String);
                        return NULL;
                    }
                }
                [pages addObject:text ?: @""];
            }
        }
        NSData *data = [NSJSONSerialization dataWithJSONObject:pages options:0 error:nil];
        if (!data) {
            *error = strdup("无法编码页面文字");
            return NULL;
        }
        return strndup(data.bytes, data.length);
    }
}
